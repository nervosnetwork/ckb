# CKB JSON-RPC Protocols

<!--**NOTE:** This file is auto-generated from code comments.-->

The RPC interface shares the version of the node version, which is returned in `local_node_info`. The interface is fully compatible between patch versions, for example, a client for 0.25.0 should work with 0.25.x for any x.

Allowing arbitrary machines to access the JSON-RPC port (using the `rpc.listen_address` configuration option) is **dangerous and strongly discouraged**. Please strictly limit the access to only trusted machines.

CKB JSON-RPC only supports HTTP now. If you need SSL, please set up a proxy via Nginx or other HTTP servers.

Subscriptions require a full duplex connection. CKB offers such connections in the form of TCP (enable with `rpc.tcp_listen_address` configuration option) and WebSockets (enable with `rpc.ws_listen_address`).

## JSONRPC Deprecation Process

A CKB RPC method is deprecated in three steps.

First, the method is marked as deprecated in the CKB release notes and RPC document. However, the RPC method is still available. The RPC document will have the suggestion of alternative solutions.

The CKB dev team will disable any deprecated RPC methods starting from the next minor version release. Users can enable the deprecated methods via the config file option rpc.enable_deprecated_rpc.

Once a deprecated method is disabled, the CKB dev team will remove it in a future minor version release.

For example, a method is marked as deprecated in 0.35.0, it can be disabled in 0.36.0 and removed in 0.37.0. The minor versions are released monthly, so there's at least a two-month buffer for a deprecated RPC method.

## Minimum Supported Rust Version policy (MSRV)

The crate `ckb-rpc`'s minimum supported rustc version is 1.61.0.


## Table of Contents

* [RPC Methods](#rpc-methods)
    * [Module Alert](#module-alert)
        * [Method `send_alert`](#method-send_alert)
    * [Module Chain](#module-chain)
        * [Method `get_block`](#method-get_block)
        * [Method `get_block_by_number`](#method-get_block_by_number)
        * [Method `get_header`](#method-get_header)
        * [Method `get_header_by_number`](#method-get_header_by_number)
        * [Method `get_block_filter`](#method-get_block_filter)
        * [Method `get_transaction`](#method-get_transaction)
        * [Method `get_block_hash`](#method-get_block_hash)
        * [Method `get_tip_header`](#method-get_tip_header)
        * [Method `get_live_cell`](#method-get_live_cell)
        * [Method `get_tip_block_number`](#method-get_tip_block_number)
        * [Method `get_current_epoch`](#method-get_current_epoch)
        * [Method `get_epoch_by_number`](#method-get_epoch_by_number)
        * [Method `get_block_economic_state`](#method-get_block_economic_state)
        * [Method `get_transaction_proof`](#method-get_transaction_proof)
        * [Method `verify_transaction_proof`](#method-verify_transaction_proof)
        * [Method `get_fork_block`](#method-get_fork_block)
        * [Method `get_consensus`](#method-get_consensus)
        * [Method `get_block_median_time`](#method-get_block_median_time)
        * [Method `estimate_cycles`](#method-estimate_cycles)
        * [Method `get_fee_rate_statics`](#method-get_fee_rate_statics)
    * [Module Experiment](#module-experiment)
        * [Method `dry_run_transaction`](#method-dry_run_transaction)
        * [Method `calculate_dao_maximum_withdraw`](#method-calculate_dao_maximum_withdraw)
    * [Module Indexer](#module-indexer)
        * [Method `get_indexer_tip`](#method-get_indexer_tip)
        * [Method `get_cells`](#method-get_cells)
        * [Method `get_transactions`](#method-get_transactions)
        * [Method `get_cells_capacity`](#method-get_cells_capacity)
    * [Module IntegrationTest](#module-integrationtest)
        * [Method `process_block_without_verify`](#method-process_block_without_verify)
        * [Method `truncate`](#method-truncate)
        * [Method `generate_block`](#method-generate_block)
        * [Method `notify_transaction`](#method-notify_transaction)
        * [Method `generate_block_with_template`](#method-generate_block_with_template)
        * [Method `calculate_dao_field`](#method-calculate_dao_field)
    * [Module Miner](#module-miner)
        * [Method `get_block_template`](#method-get_block_template)
        * [Method `submit_block`](#method-submit_block)
    * [Module Net](#module-net)
        * [Method `local_node_info`](#method-local_node_info)
        * [Method `get_peers`](#method-get_peers)
        * [Method `get_banned_addresses`](#method-get_banned_addresses)
        * [Method `clear_banned_addresses`](#method-clear_banned_addresses)
        * [Method `set_ban`](#method-set_ban)
        * [Method `sync_state`](#method-sync_state)
        * [Method `set_network_active`](#method-set_network_active)
        * [Method `add_node`](#method-add_node)
        * [Method `remove_node`](#method-remove_node)
        * [Method `ping_peers`](#method-ping_peers)
    * [Module Pool](#module-pool)
        * [Method `send_transaction`](#method-send_transaction)
        * [Method `remove_transaction`](#method-remove_transaction)
        * [Method `tx_pool_info`](#method-tx_pool_info)
        * [Method `clear_tx_pool`](#method-clear_tx_pool)
        * [Method `get_raw_tx_pool`](#method-get_raw_tx_pool)
        * [Method `tx_pool_ready`](#method-tx_pool_ready)
    * [Module Stats](#module-stats)
        * [Method `get_blockchain_info`](#method-get_blockchain_info)
        * [Method `get_deployments_info`](#method-get_deployments_info)
    * [Module Subscription](#module-subscription)
        * [Method `subscribe`](#method-subscribe)
        * [Method `unsubscribe`](#method-unsubscribe)
* [RPC Errors](#rpc-errors)
* [RPC Types](#rpc-types)
    * [Type `Alert`](#type-alert)
    * [Type `AlertId`](#type-alertid)
    * [Type `AlertMessage`](#type-alertmessage)
    * [Type `AlertPriority`](#type-alertpriority)
    * [Type `BannedAddr`](#type-bannedaddr)
    * [Type `Block`](#type-block)
    * [Type `BlockEconomicState`](#type-blockeconomicstate)
    * [Type `BlockIssuance`](#type-blockissuance)
    * [Type `BlockNumber`](#type-blocknumber)
    * [Type `BlockResponse`](#type-blockresponse)
    * [Type `BlockTemplate`](#type-blocktemplate)
    * [Type `BlockView`](#type-blockview)
    * [Type `Byte32`](#type-byte32)
    * [Type `Capacity`](#type-capacity)
    * [Type `CellData`](#type-celldata)
    * [Type `CellDep`](#type-celldep)
    * [Type `CellInfo`](#type-cellinfo)
    * [Type `CellInput`](#type-cellinput)
    * [Type `CellOutput`](#type-celloutput)
    * [Type `CellWithStatus`](#type-cellwithstatus)
    * [Type `CellbaseTemplate`](#type-cellbasetemplate)
    * [Type `ChainInfo`](#type-chaininfo)
    * [Type `Consensus`](#type-consensus)
    * [Type `Cycle`](#type-cycle)
    * [Type `DaoWithdrawingCalculationKind`](#type-daowithdrawingcalculationkind)
    * [Type `DepType`](#type-deptype)
    * [Type `DeploymentInfo`](#type-deploymentinfo)
    * [Type `DeploymentPos`](#type-deploymentpos)
    * [Type `DeploymentState`](#type-deploymentstate)
    * [Type `DeploymentsInfo`](#type-deploymentsinfo)
    * [Type `Either`](#type-either)
    * [Type `EpochNumber`](#type-epochnumber)
    * [Type `EpochNumberWithFraction`](#type-epochnumberwithfraction)
    * [Type `EpochView`](#type-epochview)
    * [Type `EstimateCycles`](#type-estimatecycles)
    * [Type `FeeRateStatics`](#type-feeratestatics)
    * [Type `H256`](#type-h256)
    * [Type `HardForkFeature`](#type-hardforkfeature)
    * [Type `Header`](#type-header)
    * [Type `HeaderView`](#type-headerview)
    * [Type `IndexerCell`](#type-indexercell)
    * [Type `IndexerCellsCapacity`](#type-indexercellscapacity)
    * [Type `IndexerOrder`](#type-indexerorder)
    * [Type `IndexerRange`](#type-indexerrange)
    * [Type `IndexerScriptType`](#type-indexerscripttype)
    * [Type `IndexerSearchKey`](#type-indexersearchkey)
    * [Type `IndexerSearchKeyFilter`](#type-indexersearchkeyfilter)
    * [Type `IndexerTip`](#type-indexertip)
    * [Type `IndexerTx`](#type-indexertx)
    * [Type `JsonBytes`](#type-jsonbytes)
    * [Type `LocalNode`](#type-localnode)
    * [Type `LocalNodeProtocol`](#type-localnodeprotocol)
    * [Type `MerkleProof`](#type-merkleproof)
    * [Type `MinerReward`](#type-minerreward)
    * [Type `NodeAddress`](#type-nodeaddress)
    * [Type `OutPoint`](#type-outpoint)
    * [Type `OutputsValidator`](#type-outputsvalidator)
    * [Type `PeerSyncState`](#type-peersyncstate)
    * [Type `PoolTransactionEntry`](#type-pooltransactionentry)
    * [Type `PoolTransactionReject`](#type-pooltransactionreject)
    * [Type `ProposalShortId`](#type-proposalshortid)
    * [Type `ProposalWindow`](#type-proposalwindow)
    * [Type `RationalU256`](#type-rationalu256)
    * [Type `RawTxPool`](#type-rawtxpool)
    * [Type `RemoteNode`](#type-remotenode)
    * [Type `RemoteNodeProtocol`](#type-remotenodeprotocol)
    * [Type `ResponseFormat`](#type-responseformat)
    * [Type `Script`](#type-script)
    * [Type `ScriptHashType`](#type-scripthashtype)
    * [Type `SerializedBlock`](#type-serializedblock)
    * [Type `SerializedHeader`](#type-serializedheader)
    * [Type `Status`](#type-status)
    * [Type `SyncState`](#type-syncstate)
    * [Type `Timestamp`](#type-timestamp)
    * [Type `Transaction`](#type-transaction)
    * [Type `TransactionProof`](#type-transactionproof)
    * [Type `TransactionTemplate`](#type-transactiontemplate)
    * [Type `TransactionView`](#type-transactionview)
    * [Type `TransactionWithStatusResponse`](#type-transactionwithstatusresponse)
    * [Type `TxPoolEntries`](#type-txpoolentries)
    * [Type `TxPoolEntry`](#type-txpoolentry)
    * [Type `TxPoolIds`](#type-txpoolids)
    * [Type `TxPoolInfo`](#type-txpoolinfo)
    * [Type `TxStatus`](#type-txstatus)
    * [Type `U256`](#type-u256)
    * [Type `Uint128`](#type-uint128)
    * [Type `Uint32`](#type-uint32)
    * [Type `Uint64`](#type-uint64)
    * [Type `UncleBlock`](#type-uncleblock)
    * [Type `UncleBlockView`](#type-uncleblockview)
    * [Type `UncleTemplate`](#type-uncletemplate)
    * [Type `Version`](#type-version)

## RPC Methods

### Module Alert

RPC Module Alert for network alerts.

An alert is a message about critical problems to be broadcast to all nodes via the p2p network.

The alerts must be signed by 2-of-4 signatures, where the public keys are hard-coded in the source code and belong to early CKB developers.

#### Method `send_alert`
* `send_alert(alert)`
    * `alert`: [`Alert`](#type-alert)
* result: `null`

Sends an alert.

This RPC returns `null` on success.

###### Errors

*   [`AlertFailedToVerifySignatures (-1000)`](#error-alertfailedtoverifysignatures) - Some signatures in the request are invalid.

*   [`P2PFailedToBroadcast (-101)`](#error-p2pfailedtobroadcast) - Alert is saved locally but has failed to broadcast to the P2P network.

*   `InvalidParams (-32602)` - The time specified in `alert.notice_until` must be in the future.

###### Examples

Request


```
{
  "jsonrpc": "2.0",
  "method": "send_alert",
  "params": [
    {
      "id": "0x1",
      "cancel": "0x0",
      "priority": "0x1",
      "message": "An example alert message!",
      "notice_until": "0x24bcca57c00",
      "signatures": [
        "0xbd07059aa9a3d057da294c2c4d96fa1e67eeb089837c87b523f124239e18e9fc7d11bb95b720478f7f937d073517d0e4eb9a91d12da5c88a05f750362f4c214dd0",
        "0x0242ef40bb64fe3189284de91f981b17f4d740c5e24a3fc9b70059db6aa1d198a2e76da4f84ab37549880d116860976e0cf81cd039563c452412076ebffa2e4453"
      ]
    }
  ],
  "id": 42
}
```


Response


```
{
  "error": {
    "code": -1000,
    "data": "SigNotEnough",
    "message":"AlertFailedToVerifySignatures: The count of sigs less than threshold."
  },
  "jsonrpc": "2.0",
  "result": null,
  "id": 42
}
```


### Module Chain

RPC Module Chain for methods related to the canonical chain.

This module queries information about the canonical chain.

##### Canonical Chain

A canonical chain is the one with the most accumulated work. The accumulated work is the sum of difficulties of all the blocks in the chain.

##### Chain Reorganization

Chain Reorganization happens when CKB found a chain that has accumulated more work than the canonical chain. The reorganization reverts the blocks in the current canonical chain if needed, and switch the canonical chain to that better chain.

##### Live Cell

A cell is live if

*   it is found as an output in any transaction in the [canonical chain](#canonical-chain), and

*   it is not found as an input in any transaction in the canonical chain.

#### Method `get_block`
* `get_block(block_hash, verbosity, with_cycles)`
    * `block_hash`: [`H256`](#type-h256)
    * `verbosity`: [`Uint32`](#type-uint32) `|` `null`
    * `with_cycles`: `boolean` `|` `null`
* result: [`BlockResponse`](#type-blockresponse) `|` `null`

Returns the information about a block by hash.

###### Params

*   `block_hash` - the block hash.

*   `verbosity` - result format which allows 0 and 2. (**Optional**, the default is 2.)

*   `with_cycles` - whether the return cycles of block transactions. (**Optional**, default false.)

###### Returns

The RPC returns a block or null. When the RPC returns a block, the block hash must equal to the parameter `block_hash`.

If the block is in the [canonical chain](#canonical-chain), the RPC must return the block information. Otherwise, the behavior is undefined. The RPC may return blocks found in local storage or simply returns null for all blocks that are not in the canonical chain. And because of [chain reorganization](#chain-reorganization), for the same `block_hash`, the RPC may sometimes return null and sometimes return the block.

When `verbosity` is 2, it returns a JSON object as the `result`. See `BlockView` for the schema.

When `verbosity` is 0, it returns a 0x-prefixed hex string as the `result`. The string encodes the block serialized by molecule using schema `table Block`.

###### Examples

Request


```
{
  "id": 42,
  "jsonrpc": "2.0",
  "method": "get_block",
  "params": [
     "0xa5f5c85987a15de25661e5a214f2c1449cd803f071acc7999820f25246471f40"
  ]
}
```


Response


```
{
  "id": 42,
  "jsonrpc": "2.0",
  "result": {
    "header": {
      "compact_target": "0x1e083126",
      "dao": "0xb5a3e047474401001bc476b9ee573000c0c387962a38000000febffacf030000",
      "epoch": "0x7080018000001",
      "extra_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
      "hash": "0xa5f5c85987a15de25661e5a214f2c1449cd803f071acc7999820f25246471f40",
      "nonce": "0x0",
      "number": "0x400",
      "parent_hash": "0xae003585fa15309b30b31aed3dcf385e9472c3c3e93746a6c4540629a6a1ed2d",
      "proposals_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
      "timestamp": "0x5cd2b117",
      "transactions_root": "0xc47d5b78b3c4c4c853e2a32810818940d0ee403423bea9ec7b8e566d9595206c",
      "version": "0x0"
    },
    "proposals": [],
    "transactions": [
      {
        "cell_deps": [],
        "hash": "0x365698b50ca0da75dca2c87f9e7b563811d3b5813736b8cc62cc3b106faceb17",
        "header_deps": [],
        "inputs": [
          {
            "previous_output": {
              "index": "0xffffffff",
              "tx_hash": "0x0000000000000000000000000000000000000000000000000000000000000000"
            },
            "since": "0x400"
          }
        ],
        "outputs": [
          {
            "capacity": "0x18e64b61cf",
            "lock": {
              "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
              "hash_type": "data",
              "args": "0x"
            },
            "type": null
          }
        ],
        "outputs_data": [
          "0x"
        ],
        "version": "0x0",
        "witnesses": [
          "0x450000000c000000410000003500000010000000300000003100000028e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5000000000000000000"
        ]
      }
    ],
    "uncles": []
  }
}
```


The response looks like below when `verbosity` is 0.


```
{
  "id": 42,
  "jsonrpc": "2.0",
  "result": "0x..."
}
```


When specifying with_cycles, the response object will be different like below:


```
{
    "id": 42,
    "jsonrpc": "2.0",
    "result": {
        "block": <Object> or "0x...",
        "cycles": []
    }
}
```


#### Method `get_block_by_number`
* `get_block_by_number(block_number, verbosity, with_cycles)`
    * `block_number`: [`BlockNumber`](#type-blocknumber)
    * `verbosity`: [`Uint32`](#type-uint32) `|` `null`
    * `with_cycles`: `boolean` `|` `null`
* result: [`BlockResponse`](#type-blockresponse) `|` `null`

Returns the block in the [canonical chain](#canonical-chain) with the specific block number.

###### Params

*   `block_number` - the block number.

*   `verbosity` - result format which allows 0 and 2. (**Optional**, the default is 2.)

*   `with_cycles` - whether the return cycles of block transactions. (**Optional**, default false.)

###### Returns

The RPC returns the block when `block_number` is less than or equal to the tip block number returned by [`get_tip_block_number`](#method-get_tip_block_number) and returns null otherwise.

Because of [chain reorganization](#chain-reorganization), the PRC may return null or even different blocks in different invocations with the same `block_number`.

When `verbosity` is 2, it returns a JSON object as the `result`. See `BlockView` for the schema.

When `verbosity` is 0, it returns a 0x-prefixed hex string as the `result`. The string encodes the block serialized by molecule using schema `table Block`.

###### Errors

*   [`ChainIndexIsInconsistent (-201)`](#error-chainindexisinconsistent) - The index is inconsistent. It says a block hash is in the main chain, but cannot read it from the database.

*   [`DatabaseIsCorrupt (-202)`](#error-databaseiscorrupt) - The data read from database is dirty. Please report it as a bug.

###### Examples

Request


```
{
  "id": 42,
  "jsonrpc": "2.0",
  "method": "get_block_by_number",
  "params": [
    "0x400"
  ]
}
```


Response


```
{
  "id": 42,
  "jsonrpc": "2.0",
  "result": {
    "header": {
      "compact_target": "0x1e083126",
      "dao": "0xb5a3e047474401001bc476b9ee573000c0c387962a38000000febffacf030000",
      "epoch": "0x7080018000001",
      "extra_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
      "hash": "0xa5f5c85987a15de25661e5a214f2c1449cd803f071acc7999820f25246471f40",
      "nonce": "0x0",
      "number": "0x400",
      "parent_hash": "0xae003585fa15309b30b31aed3dcf385e9472c3c3e93746a6c4540629a6a1ed2d",
      "proposals_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
      "timestamp": "0x5cd2b117",
      "transactions_root": "0xc47d5b78b3c4c4c853e2a32810818940d0ee403423bea9ec7b8e566d9595206c",
      "version": "0x0"
    },
    "proposals": [],
    "transactions": [
      {
        "cell_deps": [],
        "hash": "0x365698b50ca0da75dca2c87f9e7b563811d3b5813736b8cc62cc3b106faceb17",
        "header_deps": [],
        "inputs": [
          {
            "previous_output": {
              "index": "0xffffffff",
              "tx_hash": "0x0000000000000000000000000000000000000000000000000000000000000000"
            },
            "since": "0x400"
          }
        ],
        "outputs": [
          {
            "capacity": "0x18e64b61cf",
            "lock": {
              "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
              "hash_type": "data",
              "args": "0x"
            },
            "type": null
          }
        ],
        "outputs_data": [
          "0x"
        ],
        "version": "0x0",
        "witnesses": [
          "0x450000000c000000410000003500000010000000300000003100000028e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5000000000000000000"
        ]
      }
    ],
    "uncles": []
  }
}
```


The response looks like below when `verbosity` is 0.


```
{
  "id": 42,
  "jsonrpc": "2.0",
  "result": "0x..."
}
```


When specifying with_cycles, the response object will be different like below:


```
{
    "id": 42,
    "jsonrpc": "2.0",
    "result": {
        "block": <Object> or "0x...",
        "cycles": []
    }
}
```


#### Method `get_header`
* `get_header(block_hash, verbosity)`
    * `block_hash`: [`H256`](#type-h256)
    * `verbosity`: [`Uint32`](#type-uint32) `|` `null`
* result: [`HeaderView`](#type-headerview) `|` [`SerializedHeader`](#type-serializedheader) `|` `null`

Returns the information about a block header by hash.

###### Params

*   `block_hash` - the block hash.

*   `verbosity` - result format which allows 0 and 1. (**Optional**, the default is 1.)

###### Returns

The RPC returns a header or null. When the RPC returns a header, the block hash must equal to the parameter `block_hash`.

If the block is in the [canonical chain](#canonical-chain), the RPC must return the header information. Otherwise, the behavior is undefined. The RPC may return blocks found in local storage or simply returns null for all blocks that are not in the canonical chain. And because of [chain reorganization](#chain-reorganization), for the same `block_hash`, the RPC may sometimes return null and sometimes return the block header.

When `verbosity` is 1, it returns a JSON object as the `result`. See `HeaderView` for the schema.

When `verbosity` is 0, it returns a 0x-prefixed hex string as the `result`. The string encodes the block header serialized by molecule using schema `table Header`.

###### Examples

Request


```
{
  "id": 42,
  "jsonrpc": "2.0",
  "method": "get_header",
  "params": [
    "0xa5f5c85987a15de25661e5a214f2c1449cd803f071acc7999820f25246471f40"
  ]
}
```


Response


```
{
  "id": 42,
  "jsonrpc": "2.0",
  "result": {
    "compact_target": "0x1e083126",
    "dao": "0xb5a3e047474401001bc476b9ee573000c0c387962a38000000febffacf030000",
    "epoch": "0x7080018000001",
    "extra_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
    "hash": "0xa5f5c85987a15de25661e5a214f2c1449cd803f071acc7999820f25246471f40",
    "nonce": "0x0",
    "number": "0x400",
    "parent_hash": "0xae003585fa15309b30b31aed3dcf385e9472c3c3e93746a6c4540629a6a1ed2d",
    "proposals_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
    "timestamp": "0x5cd2b117",
    "transactions_root": "0xc47d5b78b3c4c4c853e2a32810818940d0ee403423bea9ec7b8e566d9595206c",
    "version": "0x0"
  }
}
```


The response looks like below when `verbosity` is 0.


```
{
  "id": 42,
  "jsonrpc": "2.0",
  "result": "0x..."
}
```


#### Method `get_header_by_number`
* `get_header_by_number(block_number, verbosity)`
    * `block_number`: [`BlockNumber`](#type-blocknumber)
    * `verbosity`: [`Uint32`](#type-uint32) `|` `null`
* result: [`HeaderView`](#type-headerview) `|` [`SerializedHeader`](#type-serializedheader) `|` `null`

Returns the block header in the [canonical chain](#canonical-chain) with the specific block number.

###### Params

*   `block_number` - Number of a block

*   `verbosity` - result format which allows 0 and 1. (**Optional**, the default is 1.)

###### Returns

The RPC returns the block header when `block_number` is less than or equal to the tip block number returned by [`get_tip_block_number`](#method-get_tip_block_number) and returns null otherwise.

Because of [chain reorganization](#chain-reorganization), the PRC may return null or even different block headers in different invocations with the same `block_number`.

When `verbosity` is 1, it returns a JSON object as the `result`. See `HeaderView` for the schema.

When `verbosity` is 0, it returns a 0x-prefixed hex string as the `result`. The string encodes the block header serialized by molecule using schema `table Header`.

###### Errors

*   [`ChainIndexIsInconsistent (-201)`](#error-chainindexisinconsistent) - The index is inconsistent. It says a block hash is in the main chain, but cannot read it from the database.

###### Examples

Request


```
{
  "id": 42,
  "jsonrpc": "2.0",
  "method": "get_header_by_number",
  "params": [
    "0x400"
  ]
}
```


Response


```
{
  "id": 42,
  "jsonrpc": "2.0",
  "result": {
    "compact_target": "0x1e083126",
    "dao": "0xb5a3e047474401001bc476b9ee573000c0c387962a38000000febffacf030000",
    "epoch": "0x7080018000001",
    "extra_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
    "hash": "0xa5f5c85987a15de25661e5a214f2c1449cd803f071acc7999820f25246471f40",
    "nonce": "0x0",
    "number": "0x400",
    "parent_hash": "0xae003585fa15309b30b31aed3dcf385e9472c3c3e93746a6c4540629a6a1ed2d",
    "proposals_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
    "timestamp": "0x5cd2b117",
    "transactions_root": "0xc47d5b78b3c4c4c853e2a32810818940d0ee403423bea9ec7b8e566d9595206c",
    "version": "0x0"
  }
}
```


The response looks like below when `verbosity` is 0.


```
{
  "id": 42,
  "jsonrpc": "2.0",
  "result": "0x..."
}
```


#### Method `get_block_filter`
* `get_block_filter(block_hash)`
    * `block_hash`: [`H256`](#type-h256)
* result: [`JsonBytes`](#type-jsonbytes) `|` `null`

Returns the block filter by block hash.

###### Params

*   `block_hash` - the block hash.

###### Returns

The block filter data

###### Examples

Request


```
{
  "id": 42,
  "jsonrpc": "2.0",
  "method": "get_block_filter",
  "params": [
    "0xa5f5c85987a15de25661e5a214f2c1449cd803f071acc7999820f25246471f40"
  ]
}
```


Response


```
{
  "id": 42,
  "jsonrpc": "2.0",
  "result": null
}
```


The response looks like below when the block have block filter.


```
{
  "id": 42,
  "jsonrpc": "2.0",
  "result": "0x..."
}
```


#### Method `get_transaction`
* `get_transaction(tx_hash, verbosity)`
    * `tx_hash`: [`H256`](#type-h256)
    * `verbosity`: [`Uint32`](#type-uint32) `|` `null`
* result: [`TransactionWithStatusResponse`](#type-transactionwithstatusresponse) `|` `null`

Returns the information about a transaction requested by transaction hash.

###### Returns

This RPC returns `null` if the transaction is not committed in the [canonical chain](#canonical-chain) nor the transaction memory pool.

If the transaction is in the chain, the block hash is also returned.

###### Params

*   `tx_hash` - Hash of a transaction

*   `verbosity` - result format which allows 0, 1 and 2. (**Optional**, the defaults to 2.)

###### Returns

When verbosity=0, it’s response value is as same as verbosity=2, but it return a 0x-prefixed hex encoded molecule packed::Transaction on `transaction` field

When verbosity is 1: The RPC does not return the transaction content and the field transaction must be null.

When verbosity is 2: if tx_status.status is pending, proposed, or committed, the RPC returns the transaction content as field transaction, otherwise the field is null.

###### Examples

Request


```
{
  "id": 42,
  "jsonrpc": "2.0",
  "method": "get_transaction",
  "params": [
    "0xa0ef4eb5f4ceeb08a4c8524d84c5da95dce2f608e0ca2ec8091191b0f330c6e3"
  ]
}
```


Response


```
{
  "id": 42,
  "jsonrpc": "2.0",
  "result": {
    "transaction": {
      "cell_deps": [
        {
          "dep_type": "code",
          "out_point": {
            "index": "0x0",
            "tx_hash": "0xa4037a893eb48e18ed4ef61034ce26eba9c585f15c9cee102ae58505565eccc3"
          }
        }
      ],
      "hash": "0xa0ef4eb5f4ceeb08a4c8524d84c5da95dce2f608e0ca2ec8091191b0f330c6e3",
      "header_deps": [
        "0x7978ec7ce5b507cfb52e149e36b1a23f6062ed150503c85bbf825da3599095ed"
      ],
      "inputs": [
        {
          "previous_output": {
            "index": "0x0",
            "tx_hash": "0x365698b50ca0da75dca2c87f9e7b563811d3b5813736b8cc62cc3b106faceb17"
          },
          "since": "0x0"
        }
      ],
      "outputs": [
        {
          "capacity": "0x2540be400",
          "lock": {
            "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
            "hash_type": "data",
            "args": "0x"
          },
          "type": null
        }
      ],
      "outputs_data": [
        "0x"
      ],
      "version": "0x0",
      "witnesses": []
    },
    "cycles": "0x219",
    "tx_status": {
      "block_hash": null,
      "status": "pending",
      "reason": null
    }
  }
}
```


The response looks like below when `verbosity` is 0.


```
{
  "id": 42,
  "jsonrpc": "2.0",
  "result": {
    "transaction": "0x.....",
    "cycles": "0x219",
    "tx_status": {
      "block_hash": null,
      "status": "pending",
      "reason": null
    }
  }
}
```


#### Method `get_block_hash`
* `get_block_hash(block_number)`
    * `block_number`: [`BlockNumber`](#type-blocknumber)
* result: [`H256`](#type-h256) `|` `null`

Returns the hash of a block in the [canonical chain](#canonical-chain) with the specified `block_number`.

###### Params

*   `block_number` - Block number

###### Returns

The RPC returns the block hash when `block_number` is less than or equal to the tip block number returned by [`get_tip_block_number`](#method-get_tip_block_number) and returns null otherwise.

Because of [chain reorganization](#chain-reorganization), the PRC may return null or even different block hashes in different invocations with the same `block_number`.

###### Examples

Request


```
{
  "id": 42,
  "jsonrpc": "2.0",
  "method": "get_block_hash",
  "params": [
    "0x400"
  ]
}
```


Response


```
{
  "id": 42,
  "jsonrpc": "2.0",
  "result": "0xa5f5c85987a15de25661e5a214f2c1449cd803f071acc7999820f25246471f40"
}
```


#### Method `get_tip_header`
* `get_tip_header(verbosity)`
    * `verbosity`: [`Uint32`](#type-uint32) `|` `null`
* result: [`HeaderView`](#type-headerview) `|` [`SerializedHeader`](#type-serializedheader)

Returns the header with the highest block number in the [canonical chain](#canonical-chain).

Because of [chain reorganization](#chain-reorganization), the block number returned can be less than previous invocations and different invocations may return different block headers with the same block number.

###### Params

*   `verbosity` - result format which allows 0 and 1. (**Optional**, the default is 1.)

###### Returns

When `verbosity` is 1, the RPC returns a JSON object as the `result`. See HeaderView for the schema.

When `verbosity` is 0, it returns a 0x-prefixed hex string as the `result`. The string encodes the header serialized by molecule using schema `table Header`.

###### Examples

Request


```
{
  "id": 42,
  "jsonrpc": "2.0",
  "method": "get_tip_header",
  "params": []
}
```


Response


```
{
  "jsonrpc": "2.0",
  "result": {
    "compact_target": "0x1e083126",
    "dao": "0xb5a3e047474401001bc476b9ee573000c0c387962a38000000febffacf030000",
    "epoch": "0x7080018000001",
    "extra_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
    "hash": "0xa5f5c85987a15de25661e5a214f2c1449cd803f071acc7999820f25246471f40",
    "nonce": "0x0",
    "number": "0x400",
    "parent_hash": "0xae003585fa15309b30b31aed3dcf385e9472c3c3e93746a6c4540629a6a1ed2d",
    "proposals_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
    "timestamp": "0x5cd2b117",
    "transactions_root": "0xc47d5b78b3c4c4c853e2a32810818940d0ee403423bea9ec7b8e566d9595206c",
    "version": "0x0"
  },
  "id": 42
}
```


The response looks like below when `verbosity` is 0.


```
{
  "id": 42,
  "jsonrpc": "2.0",
  "result": "0x..."
}
```


#### Method `get_live_cell`
* `get_live_cell(out_point, with_data)`
    * `out_point`: [`OutPoint`](#type-outpoint)
    * `with_data`: `boolean`
* result: [`CellWithStatus`](#type-cellwithstatus)

Returns the status of a cell. The RPC returns extra information if it is a [live cell](#live-cell).

###### Returns

This RPC tells whether a cell is live or not.

If the cell is live, the RPC will return details about the cell. Otherwise, the field `cell` is null in the result.

If the cell is live and `with_data` is set to `false`, the field `cell.data` is null in the result.

###### Params

*   `out_point` - Reference to the cell by transaction hash and output index.

*   `with_data` - Whether the RPC should return cell data. Cell data can be huge, if the client does not need the data, it should set this to `false` to save bandwidth.

###### Examples

Request


```
{
  "id": 42,
  "jsonrpc": "2.0",
  "method": "get_live_cell",
  "params": [
    {
      "index": "0x0",
      "tx_hash": "0xa4037a893eb48e18ed4ef61034ce26eba9c585f15c9cee102ae58505565eccc3"
    },
    true
  ]
}
```


Response


```
{
  "id": 42,
  "jsonrpc": "2.0",
  "result": {
    "cell": {
      "data": {
        "content": "0x7f454c460201010000000000000000000200f3000100000078000100000000004000000000000000980000000000000005000000400038000100400003000200010000000500000000000000000000000000010000000000000001000000000082000000000000008200000000000000001000000000000001459308d00573000000002e7368737472746162002e74657874000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000b000000010000000600000000000000780001000000000078000000000000000a0000000000000000000000000000000200000000000000000000000000000001000000030000000000000000000000000000000000000082000000000000001100000000000000000000000000000001000000000000000000000000000000",
        "hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5"
      },
      "output": {
        "capacity": "0x802665800",
        "lock": {
          "code_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
          "hash_type": "data",
          "args": "0x"
        },
        "type": null
      }
    },
    "status": "live"
  }
}
```


#### Method `get_tip_block_number`
* `get_tip_block_number()`
* result: [`BlockNumber`](#type-blocknumber)

Returns the highest block number in the [canonical chain](#canonical-chain).

Because of [chain reorganization](#chain-reorganization), the returned block number may be less than a value returned in the previous invocation.

###### Examples

Request


```
{
  "id": 42,
  "jsonrpc": "2.0",
  "method": "get_tip_block_number",
  "params": []
}
```


Response


```
{
  "id": 42,
  "jsonrpc": "2.0",
  "result": "0x400"
}
```


#### Method `get_current_epoch`
* `get_current_epoch()`
* result: [`EpochView`](#type-epochview)

Returns the epoch with the highest number in the [canonical chain](#canonical-chain).

Pay attention that like blocks with the specific block number may change because of [chain reorganization](#chain-reorganization), This RPC may return different epochs which have the same epoch number.

###### Examples

Request


```
{
  "id": 42,
  "jsonrpc": "2.0",
  "method": "get_current_epoch",
  "params": []
}
```


Response


```
{
  "id": 42,
  "jsonrpc": "2.0",
  "result": {
    "compact_target": "0x1e083126",
    "length": "0x708",
    "number": "0x1",
    "start_number": "0x3e8"
  }
}
```


#### Method `get_epoch_by_number`
* `get_epoch_by_number(epoch_number)`
    * `epoch_number`: [`EpochNumber`](#type-epochnumber)
* result: [`EpochView`](#type-epochview) `|` `null`

Returns the epoch in the [canonical chain](#canonical-chain) with the specific epoch number.

###### Params

*   `epoch_number` - Epoch number

###### Returns

The RPC returns the epoch when `epoch_number` is less than or equal to the current epoch number returned by [`get_current_epoch`](#method-get_current_epoch) and returns null otherwise.

Because of [chain reorganization](#chain-reorganization), for the same `epoch_number`, this RPC may return null or different epochs in different invocations.

###### Examples

Request


```
{
  "id": 42,
  "jsonrpc": "2.0",
  "method": "get_epoch_by_number",
  "params": [
    "0x0"
  ]
}
```


Response


```
{
  "id": 42,
  "jsonrpc": "2.0",
  "result": {
    "compact_target": "0x20010000",
    "length": "0x3e8",
    "number": "0x0",
    "start_number": "0x0"
  }
}
```


#### Method `get_block_economic_state`
* `get_block_economic_state(block_hash)`
    * `block_hash`: [`H256`](#type-h256)
* result: [`BlockEconomicState`](#type-blockeconomicstate) `|` `null`

Returns increased issuance, miner reward, and the total transaction fee of a block.

This RPC returns null if the block is not in the [canonical chain](#canonical-chain).

CKB delays CKB creation for miners. The output cells in the cellbase of block N are for the miner creating block `N - 1 - ProposalWindow.farthest`.

In mainnet, `ProposalWindow.farthest` is 10, so the outputs in block 100 are rewards for miner creating block 89.

Because of the delay, this RPC returns null if the block rewards are not finalized yet. For example, the economic state for block 89 is only available when the number returned by [`get_tip_block_number`](#method-get_tip_block_number) is greater than or equal to 100.

###### Params

*   `block_hash` - Specifies the block hash which rewards should be analyzed.

###### Returns

If the block with the hash `block_hash` is in the [canonical chain](#canonical-chain) and its rewards have been finalized, return the block rewards analysis for this block. A special case is that the return value for genesis block is null.

###### Examples

Request


```
{
  "id": 42,
  "jsonrpc": "2.0",
  "method": "get_block_economic_state",
  "params": [
    "0x02530b25ad0ff677acc365cb73de3e8cc09c7ddd58272e879252e199d08df83b"
  ]
}
```


Response


```
{
  "id": 42,
  "jsonrpc": "2.0",
  "result": {
    "finalized_at": "0xa5f5c85987a15de25661e5a214f2c1449cd803f071acc7999820f25246471f40",
    "issuance": {
      "primary": "0x18ce922bca",
      "secondary": "0x7f02ec655"
    },
    "miner_reward": {
      "committed": "0x0",
      "primary": "0x18ce922bca",
      "proposal": "0x0",
      "secondary": "0x17b93605"
    },
    "txs_fee": "0x0"
  }
}
```


#### Method `get_transaction_proof`
* `get_transaction_proof(tx_hashes, block_hash)`
    * `tx_hashes`: `Array<` [`H256`](#type-h256) `>`
    * `block_hash`: [`H256`](#type-h256) `|` `null`
* result: [`TransactionProof`](#type-transactionproof)

Returns a Merkle proof that transactions are included in a block.

###### Params

*   `tx_hashes` - Transaction hashes, all transactions must be in the same block

*   `block_hash` - An optional parameter, if specified, looks for transactions in the block with this hash

###### Examples

Request


```
{
  "id": 42,
  "jsonrpc": "2.0",
  "method": "get_transaction_proof",
  "params": [
    [ "0xa4037a893eb48e18ed4ef61034ce26eba9c585f15c9cee102ae58505565eccc3" ]
  ]
}
```


Response


```
{
  "id": 42,
  "jsonrpc": "2.0",
  "result": {
    "block_hash": "0x7978ec7ce5b507cfb52e149e36b1a23f6062ed150503c85bbf825da3599095ed",
    "proof": {
      "indices": [ "0x0" ],
      "lemmas": []
    },
    "witnesses_root": "0x2bb631f4a251ec39d943cc238fc1e39c7f0e99776e8a1e7be28a03c70c4f4853"
  }
}
```


#### Method `verify_transaction_proof`
* `verify_transaction_proof(tx_proof)`
    * `tx_proof`: [`TransactionProof`](#type-transactionproof)
* result: `Array<` [`H256`](#type-h256) `>`

Verifies that a proof points to transactions in a block, returning the transaction hashes it commits to.

###### Parameters

*   `transaction_proof` - proof generated by [`get_transaction_proof`](#method-get_transaction_proof).

###### Examples

Request


```
{
  "id": 42,
  "jsonrpc": "2.0",
  "method": "verify_transaction_proof",
  "params": [
    {
      "block_hash": "0x7978ec7ce5b507cfb52e149e36b1a23f6062ed150503c85bbf825da3599095ed",
      "proof": {
        "indices": [ "0x0" ],
        "lemmas": []
      },
      "witnesses_root": "0x2bb631f4a251ec39d943cc238fc1e39c7f0e99776e8a1e7be28a03c70c4f4853"
    }
  ]
}
```


Response


```
{
  "id": 42,
  "jsonrpc": "2.0",
  "result": [
    "0xa4037a893eb48e18ed4ef61034ce26eba9c585f15c9cee102ae58505565eccc3"
  ]
}
```


#### Method `get_fork_block`
* `get_fork_block(block_hash, verbosity)`
    * `block_hash`: [`H256`](#type-h256)
    * `verbosity`: [`Uint32`](#type-uint32) `|` `null`
* result: [`BlockView`](#type-blockview) `|` [`SerializedBlock`](#type-serializedblock) `|` `null`

Returns the information about a fork block by hash.

###### Params

*   `block_hash` - the fork block hash.

*   `verbosity` - result format which allows 0 and 2. (**Optional**, the default is 2.)

###### Returns

The RPC returns a fork block or null. When the RPC returns a block, the block hash must equal to the parameter `block_hash`.

Please note that due to the technical nature of the peer to peer sync, the RPC may return null or a fork block result on different nodes with same `block_hash` even they are fully synced to the [canonical chain](#canonical-chain). And because of [chain reorganization](#chain-reorganization), for the same `block_hash`, the RPC may sometimes return null and sometimes return the fork block.

When `verbosity` is 2, it returns a JSON object as the `result`. See `BlockView` for the schema.

When `verbosity` is 0, it returns a 0x-prefixed hex string as the `result`. The string encodes the block serialized by molecule using schema `table Block`.

###### Examples

Request


```
{
  "id": 42,
  "jsonrpc": "2.0",
  "method": "get_fork_block",
  "params": [
    "0xdca341a42890536551f99357612cef7148ed471e3b6419d0844a4e400be6ee94"
  ]
}
```


Response


```
{
  "id": 42,
  "jsonrpc": "2.0",
  "result": {
    "header": {
      "compact_target": "0x1e083126",
      "dao": "0xb5a3e047474401001bc476b9ee573000c0c387962a38000000febffacf030000",
      "epoch": "0x7080018000001",
      "extra_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
      "hash": "0xdca341a42890536551f99357612cef7148ed471e3b6419d0844a4e400be6ee94",
      "nonce": "0x0",
      "number": "0x400",
      "parent_hash": "0xae003585fa15309b30b31aed3dcf385e9472c3c3e93746a6c4540629a6a1ed2d",
      "proposals_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
      "timestamp": "0x5cd2b118",
      "transactions_root": "0xc47d5b78b3c4c4c853e2a32810818940d0ee403423bea9ec7b8e566d9595206c",
      "version": "0x0"
    },
    "proposals": [],
    "transactions": [
      {
        "cell_deps": [],
        "hash": "0x365698b50ca0da75dca2c87f9e7b563811d3b5813736b8cc62cc3b106faceb17",
        "header_deps": [],
        "inputs": [
          {
            "previous_output": {
              "index": "0xffffffff",
              "tx_hash": "0x0000000000000000000000000000000000000000000000000000000000000000"
            },
            "since": "0x400"
          }
        ],
        "outputs": [
          {
            "capacity": "0x18e64b61cf",
            "lock": {
              "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
              "hash_type": "data",
              "args": "0x"
            },
            "type": null
          }
        ],
        "outputs_data": [
          "0x"
        ],
        "version": "0x0",
        "witnesses": [
          "0x450000000c000000410000003500000010000000300000003100000028e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5000000000000000000"
        ]
      }
    ],
    "uncles": []
  }
}
```


The response looks like below when `verbosity` is 0.


```
{
  "id": 42,
  "jsonrpc": "2.0",
  "result": "0x..."
}
```


#### Method `get_consensus`
* `get_consensus()`
* result: [`Consensus`](#type-consensus)

Return various consensus parameters.

###### Returns

If any hardfork feature has `epoch=null`, it means the feature will never be activated.

###### Examples

Request


```
{
  "id": 42,
  "jsonrpc": "2.0",
  "method": "get_consensus",
  "params": []
}
```


Response


```
{
  "id": 42,
  "jsonrpc": "2.0",
  "result": {
        "block_version": "0x0",
        "cellbase_maturity": "0x10000000000",
        "dao_type_hash": null,
        "epoch_duration_target": "0x3840",
        "genesis_hash": "0x7978ec7ce5b507cfb52e149e36b1a23f6062ed150503c85bbf825da3599095ed",
        "hardfork_features": [
            { "rfc": "0028", "epoch_number": "0x1526" },
            { "rfc": "0029", "epoch_number": "0x0" },
            { "rfc": "0030", "epoch_number": "0x0" },
            { "rfc": "0031", "epoch_number": "0x0" },
            { "rfc": "0032", "epoch_number": "0x0" },
            { "rfc": "0036", "epoch_number": "0x0" },
            { "rfc": "0038", "epoch_number": "0x0" }
        ],
        "id": "main",
        "initial_primary_epoch_reward": "0x71afd498d000",
        "max_block_bytes": "0x91c08",
        "max_block_cycles": "0xd09dc300",
        "max_block_proposals_limit": "0x5dc",
        "max_uncles_num": "0x2",
        "median_time_block_count": "0x25",
        "orphan_rate_target": {
            "denom": "0x28",
            "numer": "0x1"
        },
        "permanent_difficulty_in_dummy": false,
        "primary_epoch_reward_halving_interval": "0x2238",
        "proposer_reward_ratio": {
            "denom": "0xa",
            "numer": "0x4"
        },
        "secondary_epoch_reward": "0x37d0c8e28542",
        "secp256k1_blake160_multisig_all_type_hash": null,
        "secp256k1_blake160_sighash_all_type_hash": null,
        "tx_proposal_window": {
            "closest": "0x2",
            "farthest": "0xa"
        },
        "tx_version": "0x0",
        "type_id_code_hash": "0x00000000000000000000000000000000000000000000000000545950455f4944"
    }
}
```


#### Method `get_block_median_time`
* `get_block_median_time(block_hash)`
    * `block_hash`: [`H256`](#type-h256)
* result: [`Timestamp`](#type-timestamp) `|` `null`

Returns the past median time by block hash.

###### Params

*   `block_hash` - A median time is calculated for a consecutive block sequence. `block_hash` indicates the highest block of the sequence.

###### Returns

When the given block hash is not on the current canonical chain, this RPC returns null; otherwise returns the median time of the consecutive 37 blocks where the given block_hash has the highest height.

Note that the given block is included in the median time. The included block number range is `[MAX(block - 36, 0), block]`.

###### Examples

Request


```
{
  "id": 42,
  "jsonrpc": "2.0",
  "method": "get_block_median_time",
  "params": [
    "0xa5f5c85987a15de25661e5a214f2c1449cd803f071acc7999820f25246471f40"
  ]
}
```


Response


```
{
  "id": 42,
  "jsonrpc": "2.0",
  "result": "0x5cd2b105"
}
```


#### Method `estimate_cycles`
* `estimate_cycles(tx)`
    * `tx`: [`Transaction`](#type-transaction)
* result: [`EstimateCycles`](#type-estimatecycles)

`estimate_cycles` run a transaction and return the execution consumed cycles.

This method will not check the transaction validity, but only run the lock script and type script and then return the execution cycles.

It is used to estimate how many cycles the scripts consume.

###### Errors

*   [`TransactionFailedToResolve (-301)`](#error-transactionfailedtoresolve) - Failed to resolve the referenced cells and headers used in the transaction, as inputs or dependencies.

*   [`TransactionFailedToVerify (-302)`](#error-transactionfailedtoverify) - There is a script returns with an error.

###### Examples

Request


```
{
  "id": 42,
  "jsonrpc": "2.0",
  "method": "estimate_cycles",
  "params": [
    {
      "cell_deps": [
        {
          "dep_type": "code",
          "out_point": {
            "index": "0x0",
            "tx_hash": "0xa4037a893eb48e18ed4ef61034ce26eba9c585f15c9cee102ae58505565eccc3"
          }
        }
      ],
      "header_deps": [
        "0x7978ec7ce5b507cfb52e149e36b1a23f6062ed150503c85bbf825da3599095ed"
      ],
      "inputs": [
        {
          "previous_output": {
            "index": "0x0",
            "tx_hash": "0x365698b50ca0da75dca2c87f9e7b563811d3b5813736b8cc62cc3b106faceb17"
          },
          "since": "0x0"
        }
      ],
      "outputs": [
        {
          "capacity": "0x2540be400",
          "lock": {
            "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
            "hash_type": "data",
            "args": "0x"
          },
          "type": null
        }
      ],
      "outputs_data": [
        "0x"
      ],
      "version": "0x0",
      "witnesses": []
    }
  ]
}
```


Response


```
{
  "id": 42,
  "jsonrpc": "2.0",
  "result": {
    "cycles": "0x219"
  }
}
```


#### Method `get_fee_rate_statics`
* `get_fee_rate_statics(target)`
    * `target`: [`Uint64`](#type-uint64) `|` `null`
* result: [`FeeRateStatics`](#type-feeratestatics) `|` `null`

Returns the fee_rate statistics of confirmed blocks on the chain

###### Params

*   `target` - Specify the number (1 - 101) of confirmed blocks to be counted. If the number is even, automatically add one. If not specified, defaults to 21

###### Returns

If the query finds the corresponding historical data, the corresponding statistics are returned, containing the mean and median, in shannons per kilo-weight. If not, it returns null.

###### Examples

Request


```
{
  "id": 42,
  "jsonrpc": "2.0",
  "method": "get_fee_rate_statics",
  "params": []
}
```


Response


```
{
  "id": 42,
  "jsonrpc": "2.0",
  "result": {
    "mean": "0xe79d",
    "median": "0x14a8"
   }
}
```


### Module Experiment

RPC Module Experiment for experimenting methods.

**EXPERIMENTAL warning**

The methods here may be removed or changed in future releases without prior notifications.

#### Method `dry_run_transaction`
* `dry_run_transaction(tx)`
    * `tx`: [`Transaction`](#type-transaction)
* result: [`EstimateCycles`](#type-estimatecycles)

👎 Deprecated since 0.105.1:
Please use the RPC method [`estimate_cycles`](#method-estimate_cycles) instead


Dry run a transaction and return the execution cycles.

This method will not check the transaction validity, but only run the lock script and type script and then return the execution cycles.

It is used to debug transaction scripts and query how many cycles the scripts consume.

###### Errors

*   [`TransactionFailedToResolve (-301)`](#error-transactionfailedtoresolve) - Failed to resolve the referenced cells and headers used in the transaction, as inputs or dependencies.

*   [`TransactionFailedToVerify (-302)`](#error-transactionfailedtoverify) - There is a script returns with an error.

###### Examples

Request


```
{
  "id": 42,
  "jsonrpc": "2.0",
  "method": "dry_run_transaction",
  "params": [
    {
      "cell_deps": [
        {
          "dep_type": "code",
          "out_point": {
            "index": "0x0",
            "tx_hash": "0xa4037a893eb48e18ed4ef61034ce26eba9c585f15c9cee102ae58505565eccc3"
          }
        }
      ],
      "header_deps": [
        "0x7978ec7ce5b507cfb52e149e36b1a23f6062ed150503c85bbf825da3599095ed"
      ],
      "inputs": [
        {
          "previous_output": {
            "index": "0x0",
            "tx_hash": "0x365698b50ca0da75dca2c87f9e7b563811d3b5813736b8cc62cc3b106faceb17"
          },
          "since": "0x0"
        }
      ],
      "outputs": [
        {
          "capacity": "0x2540be400",
          "lock": {
            "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
            "hash_type": "data",
            "args": "0x"
          },
          "type": null
        }
      ],
      "outputs_data": [
        "0x"
      ],
      "version": "0x0",
      "witnesses": []
    }
  ]
}
```


Response


```
{
  "id": 42,
  "jsonrpc": "2.0",
  "result": {
    "cycles": "0x219"
  }
}
```


#### Method `calculate_dao_maximum_withdraw`
* `calculate_dao_maximum_withdraw(out_point, kind)`
    * `out_point`: [`OutPoint`](#type-outpoint)
    * `kind`: [`DaoWithdrawingCalculationKind`](#type-daowithdrawingcalculationkind)
* result: [`Capacity`](#type-capacity)

Calculates the maximum withdrawal one can get, given a referenced DAO cell, and a withdrawing block hash.

###### Params

*   `out_point` - Reference to the DAO cell, the depositing transaction’s output.

*   `kind` - Two kinds of dao withdrawal amount calculation option.

option 1, the assumed reference block hash for withdrawing phase 1 transaction, this block must be in the [canonical chain](#canonical-chain), the calculation of occupied capacity will be based on the depositing transaction’s output, assuming the output of phase 1 transaction is the same as the depositing transaction’s output.

option 2, the out point of the withdrawing phase 1 transaction, the calculation of occupied capacity will be based on corresponding phase 1 transaction’s output.

###### Returns

The RPC returns the final capacity when the cell `out_point` is withdrawn using the block hash or withdrawing phase 1 transaction out point as the reference.

In CKB, scripts cannot get the information about in which block the transaction is committed. A workaround is letting the transaction reference a block hash so the script knows that the transaction is committed at least after the reference block.

###### Errors

*   [`DaoError (-5)`](#error-daoerror) - The given out point is not a valid cell for DAO computation.

*   [`CKBInternalError (-1)`](#error-ckbinternalerror) - Mathematics overflow.

###### Examples

Request


```
{
  "id": 42,
  "jsonrpc": "2.0",
  "method": "calculate_dao_maximum_withdraw",
  "params": [
    {
      "index": "0x0",
      "tx_hash": "0xa4037a893eb48e18ed4ef61034ce26eba9c585f15c9cee102ae58505565eccc3"
    },
    "0xa5f5c85987a15de25661e5a214f2c1449cd803f071acc7999820f25246471f40"
  ]
}
```


Response


```
{
  "id": 42,
  "jsonrpc": "2.0",
  "result": "0x4a8b4e8a4"
}
```


### Module Indexer

RPC Module Indexer.

#### Method `get_indexer_tip`
* `get_indexer_tip()`
* result: [`IndexerTip`](#type-indexertip) `|` `null`

Returns the indexed tip

###### Returns

*   block_hash - indexed tip block hash

*   block_number - indexed tip block number

###### Examples

Request


```
{
    "id": 2,
    "jsonrpc": "2.0",
    "method": "get_indexer_tip"
}
```


Response


```
{
  "jsonrpc": "2.0",
  "result": {
    "block_hash": "0x4959d6e764a2edc6038dbf03d61ebcc99371115627b186fdcccb2161fbd26edc",
    "block_number": "0x5b513e"
  },
  "id": 2
}
```


#### Method `get_cells`
* `get_cells(search_key, order, limit, after)`
    * `search_key`: [`IndexerSearchKey`](#type-indexersearchkey)
    * `order`: [`IndexerOrder`](#type-indexerorder)
    * `limit`: [`Uint32`](#type-uint32)
    * `after`: [`JsonBytes`](#type-jsonbytes) `|` `null`
* result: `IndexerPagination<` [`IndexerCell`](#type-indexercell) `>`

Returns the live cells collection by the lock or type script.

###### Params

*   search_key:
    *   script - Script, supports prefix search

    *   scrip_type - enum, lock | type

    *   filter - filter cells by following conditions, all conditions are optional
        *   script: if search script type is lock, filter cells by type script prefix, and vice versa

        *   script_len_range: [u64; 2], filter cells by script len range, [inclusive, exclusive]

        *   output_data_len_range: [u64; 2], filter cells by output data len range, [inclusive, exclusive]

        *   output_capacity_range: [u64; 2], filter cells by output capacity range, [inclusive, exclusive]

        *   block_range: [u64; 2], filter cells by block number range, [inclusive, exclusive]


    *   with_data - bool, optional default is `true`, if with_data is set to false, the field of returning cell.output_data is null in the result


*   order: enum, asc | desc

*   limit: result size limit

*   after_cursor: pagination parameter, optional

###### Returns

*   objects:
    *   output: the fields of an output cell

    *   output_data: the cell data

    *   out_point: reference to a cell via transaction hash and output index

    *   block_number: the number of the transaction committed in the block

    *   tx_index: the position index of the transaction committed in the block


*   last_cursor: pagination parameter

###### Examples

*   get cells by lock script

Request


```
{
    "id": 2,
    "jsonrpc": "2.0",
    "method": "get_cells",
    "params": [
        {
            "script": {
                "code_hash": "0x9bd7e06f3ecf4be0f2fcd2188b23f1b9fcc88e5d4b65a8637b17723bbda3cce8",
                "hash_type": "type",
                "args": "0x5989ae415bb667931a99896e5fbbfad9ba53a223"
            },
            "script_type": "lock"
        },
        "asc",
        "0x64"
    ]
}
```


Response


```
   {
    "jsonrpc": "2.0",
    "result": {
      "last_cursor": "0x409bd7e06f3ecf4be0f2fcd2188b23f1b9fcc88e5d4b65a8637b17723bbda3cce8015989ae415bb667931a99896e5fbbfad9ba53a22300000000005b0f8c0000000100000000",
      "objects": [
        {
          "block_number": "0x5b0e6d",
          "out_point": {
            "index": "0x0",
            "tx_hash": "0xe8f2180dfba0cb15b45f771d520834515a5f8d7aa07f88894da88c22629b79e9"
          },
          "output": {
            "capacity": "0x189640200",
            "lock": {
              "args": "0x5989ae415bb667931a99896e5fbbfad9ba53a223",
              "code_hash": "0x9bd7e06f3ecf4be0f2fcd2188b23f1b9fcc88e5d4b65a8637b17723bbda3cce8",
              "hash_type": "type"
            },
            "type": null
          },
          "output_data": "0x",
          "tx_index": "0x1"
        },
        {
          "block_number": "0x5b0e90",
          "out_point": {
            "index": "0x0",
            "tx_hash": "0xece3a27409bde2914fb7a1555d6bfca453ee46af73e665149ef549fd46ec1fc6"
          },
          "output": {
            "capacity": "0x189640200",
            "lock": {
              "args": "0x5989ae415bb667931a99896e5fbbfad9ba53a223",
              "code_hash": "0x9bd7e06f3ecf4be0f2fcd2188b23f1b9fcc88e5d4b65a8637b17723bbda3cce8",
              "hash_type": "type"
            },
            "type": null
          },
          "output_data": "0x",
          "tx_index": "0x1"
        },
        {
          "block_number": "0x5b0ead",
          "out_point": {
            "index": "0x1",
            "tx_hash": "0x5c48768f91e3795b418c53211c76fd038c464a24c4aa7e35bbbb6ac5b219f581"
          },
          "output": {
            "capacity": "0xe36dceec20",
            "lock": {
              "args": "0x5989ae415bb667931a99896e5fbbfad9ba53a223",
              "code_hash": "0x9bd7e06f3ecf4be0f2fcd2188b23f1b9fcc88e5d4b65a8637b17723bbda3cce8",
              "hash_type": "type"
            },
            "type": null
          },
          "output_data": "0x",
          "tx_index": "0x1"
        },
        {
          "block_number": "0x5b0eeb",
          "out_point": {
            "index": "0x0",
            "tx_hash": "0x90e6981d6a5692d92e54344dc0e12d213447710fa069cc19ddea874619b9ba48"
          },
          "output": {
            "capacity": "0x174876e800",
            "lock": {
              "args": "0x5989ae415bb667931a99896e5fbbfad9ba53a223",
              "code_hash": "0x9bd7e06f3ecf4be0f2fcd2188b23f1b9fcc88e5d4b65a8637b17723bbda3cce8",
              "hash_type": "type"
            },
            "type": null
          },
          "output_data": "0x",
          "tx_index": "0x1"
        },
        {
          "block_number": "0x5b0f8c",
          "out_point": {
            "index": "0x0",
            "tx_hash": "0x9ea14510219ae97afa0275215fa77c3c015905281c953a3917a7fd036767429c"
          },
          "output": {
            "capacity": "0x189640200",
            "lock": {
              "args": "0x5989ae415bb667931a99896e5fbbfad9ba53a223",
              "code_hash": "0x9bd7e06f3ecf4be0f2fcd2188b23f1b9fcc88e5d4b65a8637b17723bbda3cce8",
              "hash_type": "type"
            },
            "type": null
          },
          "output_data": "0x",
          "tx_index": "0x1"
        }
      ]
    },
    "id": 2
  }
```


*   get cells by lock script and filter by type script

Request


```
{
    "id": 2,
    "jsonrpc": "2.0",
    "method": "get_cells",
    "params": [
        {
            "script": {
                "code_hash": "0x58c5f491aba6d61678b7cf7edf4910b1f5e00ec0cde2f42e0abb4fd9aff25a63",
                "hash_type": "type",
                "args": "0x2a49720e721553d0614dff29454ee4e1f07d0707"
            },
            "script_type": "lock",
            "filter": {
                "script": {
                    "code_hash": "0xc5e5dcf215925f7ef4dfaf5f4b4f105bc321c02776d6e7d52a1db3fcd9d011a4",
                    "hash_type": "type",
                    "args": "0x8462b20277bcbaa30d821790b852fb322d55c2b12e750ea91ad7059bc98dda4b"
                }
            }
        },
        "asc",
        "0x64"
    ]
}
```


Response


```
{
    "jsonrpc": "2.0",
    "result": {
      "last_cursor": "0x4058c5f491aba6d61678b7cf7edf4910b1f5e00ec0cde2f42e0abb4fd9aff25a63012a49720e721553d0614dff29454ee4e1f07d070700000000002adf870000000100000001",
      "objects": [
        {
          "block_number": "0x2adf87",
          "out_point": {
            "index": "0x1",
            "tx_hash": "0x04ecbc2df39e3682326a3b23c1bd2465e07eae2379ac0cc713834a1f79753779"
          },
          "output": {
            "capacity": "0x436d81500",
            "lock": {
              "args": "0x2a49720e721553d0614dff29454ee4e1f07d0707",
              "code_hash": "0x58c5f491aba6d61678b7cf7edf4910b1f5e00ec0cde2f42e0abb4fd9aff25a63",
              "hash_type": "type"
            },
            "type": {
              "args": "0x8462b20277bcbaa30d821790b852fb322d55c2b12e750ea91ad7059bc98dda4b",
              "code_hash": "0xc5e5dcf215925f7ef4dfaf5f4b4f105bc321c02776d6e7d52a1db3fcd9d011a4",
              "hash_type": "type"
            }
          },
          "output_data": "0x0040d20853d746000000000000000000",
          "tx_index": "0x1"
        }
      ]
    },
    "id": 2
}
```


*   get cells by lock script and filter empty type script by setting script_len_range to [0, 1), script_len is caculated by (code_hash + hash_type + args).len

Request


```
{
    "id": 2,
    "jsonrpc": "2.0",
    "method": "get_cells",
    "params": [
        {
            "script": {
                "code_hash": "0x58c5f491aba6d61678b7cf7edf4910b1f5e00ec0cde2f42e0abb4fd9aff25a63",
                "hash_type": "type",
                "args": "0x2a49720e721553d0614dff29454ee4e1f07d0707"
            },
            "script_type": "lock",
            "filter": {
                "script_len_range": ["0x0", "0x1"]
            }
        },
        "asc",
        "0x64"
    ]
}
```


Response


```
{
    "jsonrpc": "2.0",
    "result": {
      "last_cursor": "0x4058c5f491aba6d61678b7cf7edf4910b1f5e00ec0cde2f42e0abb4fd9aff25a63012a49720e721553d0614dff29454ee4e1f07d070700000000002adf830000000200000001",
      "objects": [
        {
          "block_number": "0x2adf83",
          "out_point": {
            "index": "0x1",
            "tx_hash": "0x23ec897027c1d2a2b39e2446162bac182f18581be048cb3896ad695559b6839e"
          },
          "output": {
            "capacity": "0x54b42b70b4",
            "lock": {
              "args": "0x2a49720e721553d0614dff29454ee4e1f07d0707",
              "code_hash": "0x58c5f491aba6d61678b7cf7edf4910b1f5e00ec0cde2f42e0abb4fd9aff25a63",
              "hash_type": "type"
            },
            "type": null
          },
          "output_data": "0x",
          "tx_index": "0x2"
        }
      ]
    },
    "id": 2
}
```


*   get cells by lock script and filter capacity range

Request


```
{
    "id": 2,
    "jsonrpc": "2.0",
    "method": "get_cells",
    "params": [
        {
            "script": {
                "code_hash": "0x9bd7e06f3ecf4be0f2fcd2188b23f1b9fcc88e5d4b65a8637b17723bbda3cce8",
                "hash_type": "type",
                "args": "0x5989ae415bb667931a99896e5fbbfad9ba53a223"
            },
            "script_type": "lock",
            "filter": {
                "output_capacity_range": ["0x0", "0x174876e801"]
            }
        },
        "asc",
        "0x64"
    ]
}
```


Response


```
{
    "jsonrpc": "2.0",
    "result": {
      "last_cursor": "0x409bd7e06f3ecf4be0f2fcd2188b23f1b9fcc88e5d4b65a8637b17723bbda3cce8015989ae415bb667931a99896e5fbbfad9ba53a22300000000005b59df0000000100000001",
      "objects": [
        {
          "block_number": "0x5b59df",
          "out_point": {
            "index": "0x1",
            "tx_hash": "0x21c4632a41140b828e9347ff80480b3e07be4e0a0b8d577565e7421fd5473194"
          },
          "output": {
            "capacity": "0xe815b81c0",
            "lock": {
              "args": "0x5989ae415bb667931a99896e5fbbfad9ba53a223",
              "code_hash": "0x9bd7e06f3ecf4be0f2fcd2188b23f1b9fcc88e5d4b65a8637b17723bbda3cce8",
              "hash_type": "type"
            },
            "type": null
          },
          "output_data": "0x",
          "tx_index": "0x1"
        }
      ]
    },
    "id": 2
}
```


#### Method `get_transactions`
* `get_transactions(search_key, order, limit, after)`
    * `search_key`: [`IndexerSearchKey`](#type-indexersearchkey)
    * `order`: [`IndexerOrder`](#type-indexerorder)
    * `limit`: [`Uint32`](#type-uint32)
    * `after`: [`JsonBytes`](#type-jsonbytes) `|` `null`
* result: `IndexerPagination<` [`IndexerTx`](#type-indexertx) `>`

Returns the transactions collection by the lock or type script.

*   search_key:
    *   script - Script, supports prefix search when group_by_transaction is false

    *   scrip_type - enum, lock | type

    *   filter - filter cells by following conditions, all conditions are optional
        *   script: if search script type is lock, filter cells by type script, and vice versa

        *   block_range: [u64; 2], filter cells by block number range, [inclusive, exclusive]


    *   group_by_transaction - bool, optional default is `false`, if group_by_transaction is set to true, the returning objects will be grouped by the tx hash


*   order: enum, asc | desc

*   limit: result size limit

*   after_cursor: pagination parameter, optional

###### Returns

*   objects - enum, ungrouped TxWithCell | grouped TxWithCells
    *   TxWithCell:
        *   tx_hash: transaction hash,

        *   block_number: the number of the transaction committed in the block

        *   tx_index: the position index of the transaction committed in the block

        *   io_type: enum, input | output

        *   io_index: the position index of the cell in the transaction inputs or outputs


    *   TxWithCells:
        *   tx_hash: transaction hash,

        *   block_number: the number of the transaction committed in the block

        *   tx_index: the position index of the transaction committed in the block

        *   cells: Array [[io_type, io_index]]



*   last_cursor - pagination parameter

###### Examples

*   get transactions by lock script

Request


```
{
    "id": 2,
    "jsonrpc": "2.0",
    "method": "get_transactions",
    "params": [
        {
            "script": {
                "code_hash": "0x9bd7e06f3ecf4be0f2fcd2188b23f1b9fcc88e5d4b65a8637b17723bbda3cce8",
                "hash_type": "type",
                "args": "0x5989ae415bb667931a99896e5fbbfad9ba53a223"
            },
            "script_type": "lock"
        },
        "asc",
        "0x64"
    ]
}
```


Response


```
{
    "jsonrpc": "2.0",
    "result": {
      "last_cursor": "0x809bd7e06f3ecf4be0f2fcd2188b23f1b9fcc88e5d4b65a8637b17723bbda3cce8015989ae415bb667931a99896e5fbbfad9ba53a22300000000005b59df000000010000000101",
      "objects": [
        {
          "block_number": "0x5b033a",
          "io_index": "0x0",
          "io_type": "output",
          "tx_hash": "0x556060b62d16386da53f8a4b458314dfa2d1988a7bcc5c96c3bb2a350a3453a1",
          "tx_index": "0x4"
        },
        {
          "block_number": "0x5b0671",
          "io_index": "0x0",
          "io_type": "input",
          "tx_hash": "0x8205b2b4cd6380d7e332c7a5b49bf776a0322ba19f46dc6ca1f8c59f7daee08d",
          "tx_index": "0x1"
        },
        {
          "block_number": "0x5b0671",
          "io_index": "0x1",
          "io_type": "output",
          "tx_hash": "0x8205b2b4cd6380d7e332c7a5b49bf776a0322ba19f46dc6ca1f8c59f7daee08d",
          "tx_index": "0x1"
        },
        {
          "block_number": "0x5b0e6d",
          "io_index": "0x0",
          "io_type": "output",
          "tx_hash": "0xe8f2180dfba0cb15b45f771d520834515a5f8d7aa07f88894da88c22629b79e9",
          "tx_index": "0x1"
        },
        {
          "block_number": "0x5b0e90",
          "io_index": "0x0",
          "io_type": "output",
          "tx_hash": "0xece3a27409bde2914fb7a1555d6bfca453ee46af73e665149ef549fd46ec1fc6",
          "tx_index": "0x1"
        },
        {
          "block_number": "0x5b0ead",
          "io_index": "0x0",
          "io_type": "input",
          "tx_hash": "0x5c48768f91e3795b418c53211c76fd038c464a24c4aa7e35bbbb6ac5b219f581",
          "tx_index": "0x1"
        },
        {
          "block_number": "0x5b0ead",
          "io_index": "0x1",
          "io_type": "output",
          "tx_hash": "0x5c48768f91e3795b418c53211c76fd038c464a24c4aa7e35bbbb6ac5b219f581",
          "tx_index": "0x1"
        },
        {
          "block_number": "0x5b0eeb",
          "io_index": "0x0",
          "io_type": "output",
          "tx_hash": "0x90e6981d6a5692d92e54344dc0e12d213447710fa069cc19ddea874619b9ba48",
          "tx_index": "0x1"
        },
        {
          "block_number": "0x5b0f8c",
          "io_index": "0x0",
          "io_type": "output",
          "tx_hash": "0x9ea14510219ae97afa0275215fa77c3c015905281c953a3917a7fd036767429c",
          "tx_index": "0x1"
        },
        {
          "block_number": "0x5b5638",
          "io_index": "0x0",
          "io_type": "input",
          "tx_hash": "0x9346da4caa846cc035c182ecad0c17326a587983d25fb1e12a388f1a9c5c56b4",
          "tx_index": "0x1"
        },
        {
          "block_number": "0x5b5638",
          "io_index": "0x1",
          "io_type": "input",
          "tx_hash": "0x9346da4caa846cc035c182ecad0c17326a587983d25fb1e12a388f1a9c5c56b4",
          "tx_index": "0x1"
        },
        {
          "block_number": "0x5b5638",
          "io_index": "0x1",
          "io_type": "output",
          "tx_hash": "0x9346da4caa846cc035c182ecad0c17326a587983d25fb1e12a388f1a9c5c56b4",
          "tx_index": "0x1"
        },
        {
          "block_number": "0x5b5638",
          "io_index": "0x2",
          "io_type": "input",
          "tx_hash": "0x9346da4caa846cc035c182ecad0c17326a587983d25fb1e12a388f1a9c5c56b4",
          "tx_index": "0x1"
        },
        {
          "block_number": "0x5b59c2",
          "io_index": "0x0",
          "io_type": "input",
          "tx_hash": "0x5b58f90fb3309333bf0bec878f3a05038c7fe816747300ecdac37a9da76c4128",
          "tx_index": "0x1"
        },
        {
          "block_number": "0x5b59c2",
          "io_index": "0x1",
          "io_type": "output",
          "tx_hash": "0x5b58f90fb3309333bf0bec878f3a05038c7fe816747300ecdac37a9da76c4128",
          "tx_index": "0x1"
        },
        {
          "block_number": "0x5b59cc",
          "io_index": "0x0",
          "io_type": "input",
          "tx_hash": "0x57ca2822c28e02b199424a731b2efd2c9bf752f07b7309f555f2e71abe83ba26",
          "tx_index": "0x1"
        },
        {
          "block_number": "0x5b59cc",
          "io_index": "0x1",
          "io_type": "input",
          "tx_hash": "0x57ca2822c28e02b199424a731b2efd2c9bf752f07b7309f555f2e71abe83ba26",
          "tx_index": "0x1"
        },
        {
          "block_number": "0x5b59cc",
          "io_index": "0x1",
          "io_type": "output",
          "tx_hash": "0x57ca2822c28e02b199424a731b2efd2c9bf752f07b7309f555f2e71abe83ba26",
          "tx_index": "0x1"
        },
        {
          "block_number": "0x5b59df",
          "io_index": "0x0",
          "io_type": "input",
          "tx_hash": "0x21c4632a41140b828e9347ff80480b3e07be4e0a0b8d577565e7421fd5473194",
          "tx_index": "0x1"
        },
        {
          "block_number": "0x5b59df",
          "io_index": "0x1",
          "io_type": "output",
          "tx_hash": "0x21c4632a41140b828e9347ff80480b3e07be4e0a0b8d577565e7421fd5473194",
          "tx_index": "0x1"
        }
      ]
    },
    "id": 2
}
```


*   get transactions by lock script and group by tx hash

Request


```
{
    "id": 2,
    "jsonrpc": "2.0",
    "method": "get_transactions",
    "params": [
        {
            "script": {
                "code_hash": "0x9bd7e06f3ecf4be0f2fcd2188b23f1b9fcc88e5d4b65a8637b17723bbda3cce8",
                "hash_type": "type",
                "args": "0x5989ae415bb667931a99896e5fbbfad9ba53a223"
            },
            "script_type": "lock",
            "group_by_transaction": true
        },
        "asc",
        "0x64"
    ]
}
```


Response


```
{
    "jsonrpc": "2.0",
    "result": {
      "last_cursor": "0x809bd7e06f3ecf4be0f2fcd2188b23f1b9fcc88e5d4b65a8637b17723bbda3cce8015989ae415bb667931a99896e5fbbfad9ba53a22300000000005b59df000000010000000101",
      "objects": [
        {
          "block_number": "0x5b033a",
          "cells": [
            [
              "output",
              "0x0"
            ]
          ],
          "tx_hash": "0x556060b62d16386da53f8a4b458314dfa2d1988a7bcc5c96c3bb2a350a3453a1",
          "tx_index": "0x4"
        },
        {
          "block_number": "0x5b0671",
          "cells": [
            [
              "input",
              "0x0"
            ],
            [
              "output",
              "0x1"
            ]
          ],
          "tx_hash": "0x8205b2b4cd6380d7e332c7a5b49bf776a0322ba19f46dc6ca1f8c59f7daee08d",
          "tx_index": "0x1"
        },
        {
          "block_number": "0x5b0e6d",
          "cells": [
            [
              "output",
              "0x0"
            ]
          ],
          "tx_hash": "0xe8f2180dfba0cb15b45f771d520834515a5f8d7aa07f88894da88c22629b79e9",
          "tx_index": "0x1"
        },
        {
          "block_number": "0x5b0e90",
          "cells": [
            [
              "output",
              "0x0"
            ]
          ],
          "tx_hash": "0xece3a27409bde2914fb7a1555d6bfca453ee46af73e665149ef549fd46ec1fc6",
          "tx_index": "0x1"
        },
        {
          "block_number": "0x5b0ead",
          "cells": [
            [
              "input",
              "0x0"
            ],
            [
              "output",
              "0x1"
            ]
          ],
          "tx_hash": "0x5c48768f91e3795b418c53211c76fd038c464a24c4aa7e35bbbb6ac5b219f581",
          "tx_index": "0x1"
        },
        {
          "block_number": "0x5b0eeb",
          "cells": [
            [
              "output",
              "0x0"
            ]
          ],
          "tx_hash": "0x90e6981d6a5692d92e54344dc0e12d213447710fa069cc19ddea874619b9ba48",
          "tx_index": "0x1"
        },
        {
          "block_number": "0x5b0f8c",
          "cells": [
            [
              "output",
              "0x0"
            ]
          ],
          "tx_hash": "0x9ea14510219ae97afa0275215fa77c3c015905281c953a3917a7fd036767429c",
          "tx_index": "0x1"
        },
        {
          "block_number": "0x5b5638",
          "cells": [
            [
              "input",
              "0x0"
            ],
            [
              "input",
              "0x1"
            ],
            [
              "output",
              "0x1"
            ],
            [
              "input",
              "0x2"
            ]
          ],
          "tx_hash": "0x9346da4caa846cc035c182ecad0c17326a587983d25fb1e12a388f1a9c5c56b4",
          "tx_index": "0x1"
        },
        {
          "block_number": "0x5b59c2",
          "cells": [
            [
              "input",
              "0x0"
            ],
            [
              "output",
              "0x1"
            ]
          ],
          "tx_hash": "0x5b58f90fb3309333bf0bec878f3a05038c7fe816747300ecdac37a9da76c4128",
          "tx_index": "0x1"
        },
        {
          "block_number": "0x5b59cc",
          "cells": [
            [
              "input",
              "0x0"
            ],
            [
              "input",
              "0x1"
            ],
            [
              "output",
              "0x1"
            ]
          ],
          "tx_hash": "0x57ca2822c28e02b199424a731b2efd2c9bf752f07b7309f555f2e71abe83ba26",
          "tx_index": "0x1"
        },
        {
          "block_number": "0x5b59df",
          "cells": [
            [
              "input",
              "0x0"
            ],
            [
              "output",
              "0x1"
            ]
          ],
          "tx_hash": "0x21c4632a41140b828e9347ff80480b3e07be4e0a0b8d577565e7421fd5473194",
          "tx_index": "0x1"
        }
      ]
    },
    "id": 2
}
```


#### Method `get_cells_capacity`
* `get_cells_capacity(search_key)`
    * `search_key`: [`IndexerSearchKey`](#type-indexersearchkey)
* result: [`IndexerCellsCapacity`](#type-indexercellscapacity) `|` `null`

Returns the live cells capacity by the lock or type script.

###### Parameters

*   search_key:
    *   script - Script

    *   scrip_type - enum, lock | type

    *   filter - filter cells by following conditions, all conditions are optional
        *   script: if search script type is lock, filter cells by type script prefix, and vice versa

        *   output_data_len_range: [u64; 2], filter cells by output data len range, [inclusive, exclusive]

        *   output_capacity_range: [u64; 2], filter cells by output capacity range, [inclusive, exclusive]

        *   block_range: [u64; 2], filter cells by block number range, [inclusive, exclusive]



###### Returns

*   capacity - total capacity

*   block_hash - indexed tip block hash

*   block_number - indexed tip block number

###### Examples

Request


```
{
    "id": 2,
    "jsonrpc": "2.0",
    "method": "get_cells_capacity",
    "params": [
        {
            "script": {
                "code_hash": "0x9bd7e06f3ecf4be0f2fcd2188b23f1b9fcc88e5d4b65a8637b17723bbda3cce8",
                "hash_type": "type",
                "args": "0x5989ae415bb667931a99896e5fbbfad9ba53a223"
            },
            "script_type": "lock"
        }
    ]
}
```


Response


```
{
    "jsonrpc": "2.0",
    "result": {
      "block_hash": "0xbc52444952dc5eb01a7826aaf6bb1b660db01797414e259e7a6e6d636de8fc7c",
      "block_number": "0x5b727a",
      "capacity": "0xf0e8e4b4a0"
    },
    "id": 2
}
```


### Module IntegrationTest

RPC for Integration Test.

#### Method `process_block_without_verify`
* `process_block_without_verify(data, broadcast)`
    * `data`: [`Block`](#type-block)
    * `broadcast`: `boolean`
* result: [`H256`](#type-h256) `|` `null`

process block without any block verification.

###### Params

*
    `data` - block data(in binary).


*
    `broadcast` - true to enable broadcast(relay) the block to other peers.


###### Examples

Request


```
{
  "id": 42,
  "jsonrpc": "2.0",
  "method": "process_block_without_verify",
  "params": [
   {
	"header": {
		"compact_target": "0x1e083126",
		"dao": "0xb5a3e047474401001bc476b9ee573000c0c387962a38000000febffacf030000",
		"epoch": "0x7080018000001",
		"extra_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
		"nonce": "0x0",
		"number": "0x400",
		"parent_hash": "0xae003585fa15309b30b31aed3dcf385e9472c3c3e93746a6c4540629a6a1ed2d",
		"proposals_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
		"timestamp": "0x5cd2b117",
		"transactions_root": "0xc47d5b78b3c4c4c853e2a32810818940d0ee403423bea9ec7b8e566d9595206c",
		"version": "0x0"
	},
	"proposals": [],
	"transactions": [{
		"cell_deps": [],
		"header_deps": [],
		"inputs": [{
			"previous_output": {
				"index": "0xffffffff",
				"tx_hash": "0x0000000000000000000000000000000000000000000000000000000000000000"
			},
			"since": "0x400"
		}],
		"outputs": [{
			"capacity": "0x18e64b61cf",
			"lock": {
				"code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
				"hash_type": "data",
				"args": "0x"
			},
			"type": null
		}],
		"outputs_data": [
			"0x"
		],
		"version": "0x0",
		"witnesses": [
			"0x450000000c000000410000003500000010000000300000003100000028e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5000000000000000000"
		]
	}],
	"uncles": []
    },
    true
  ]
}
```


Response


```
{
  "id": 42,
  "jsonrpc": "2.0",
  "result": "0xa5f5c85987a15de25661e5a214f2c1449cd803f071acc7999820f25246471f40",
  "error": null
}
```


#### Method `truncate`
* `truncate(target_tip_hash)`
    * `target_tip_hash`: [`H256`](#type-h256)
* result: `null`

Truncate chain to specified tip hash.

###### Params

*   `target_tip_hash` - specified header hash

###### Examples

Request


```
{
  "id": 42,
  "jsonrpc": "2.0",
  "method": "truncate",
  "params": [
    "0xa5f5c85987a15de25661e5a214f2c1449cd803f071acc7999820f25246471f40"
  ]
}
```


Response


```
{
  "id": 42,
  "jsonrpc": "2.0",
  "result": null
}
```


#### Method `generate_block`
* `generate_block()`
* result: [`H256`](#type-h256)

Generate block with block_assembler_config, process the block(with verification)

and broadcast the block.

###### Params

*
    `block_assembler_script` - specified block assembler script


*
    `block_assembler_message` - specified block assembler message


###### Examples

Request


```
{
  "id": 42,
  "jsonrpc": "2.0",
  "method": "generate_block",
  "params": []
}
```


Response


```
{
  "id": 42,
  "jsonrpc": "2.0",
  "result": "0x60dd3fa0e81db3ee3ad41cf4ab956eae7e89eb71cd935101c26c4d0652db3029",
  "error": null
}
```


#### Method `notify_transaction`
* `notify_transaction(transaction)`
    * `transaction`: [`Transaction`](#type-transaction)
* result: [`H256`](#type-h256)

Add transaction to tx-pool.

###### Params

*   `transaction` - specified transaction to add

###### Examples

Request


```
{
	"id": 42,
	"jsonrpc": "2.0",
	"method": "notify_transaction",
	"params":
    [
         {
			"cell_deps": [{
				"dep_type": "code",
				"out_point": {
					"index": "0x0",
					"tx_hash": "0xa4037a893eb48e18ed4ef61034ce26eba9c585f15c9cee102ae58505565eccc3"
				}
			}],
			"header_deps": [
				"0x7978ec7ce5b507cfb52e149e36b1a23f6062ed150503c85bbf825da3599095ed"
			],
			"inputs": [{
				"previous_output": {
					"index": "0x0",
					"tx_hash": "0x365698b50ca0da75dca2c87f9e7b563811d3b5813736b8cc62cc3b106faceb17"
				},
				"since": "0x0"
			}],
			"outputs": [{
				"capacity": "0x2540be400",
				"lock": {
					"code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
					"hash_type": "data",
					"args": "0x"
				},
				"type": null
			}],
			"outputs_data": [
				"0x"
			],
			"version": "0x0",
			"witnesses": []
		}
	]
}
```


Response


```
{
  "id": 42,
  "jsonrpc": "2.0",
  "result": "0xa0ef4eb5f4ceeb08a4c8524d84c5da95dce2f608e0ca2ec8091191b0f330c6e3",
  "error": null
}
```


#### Method `generate_block_with_template`
* `generate_block_with_template(block_template)`
    * `block_template`: [`BlockTemplate`](#type-blocktemplate)
* result: [`H256`](#type-h256)

Generate block with block template, attach calculated dao field to build new block,

then process block and broadcast the block.

###### Params

*   `block_template` - specified transaction to add

###### Examples

Request


```
{
  "id": 42,
  "jsonrpc": "2.0",
  "method": "generate_block_with_template",
  "params": [
   {
    "bytes_limit": "0x91c08",
    "cellbase": {
      "cycles": null,
      "data": {
        "cell_deps": [],
        "header_deps": [],
        "inputs": [
          {
            "previous_output": {
              "index": "0xffffffff",
              "tx_hash": "0x0000000000000000000000000000000000000000000000000000000000000000"
            },
            "since": "0x401"
          }
        ],
       "outputs": [
         {
           "capacity": "0x18e64efc04",
            "lock": {
              "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
              "hash_type": "data",
              "args": "0x"
            },
            "type": null
          }
        ],
        "outputs_data": [
          "0x"
        ],
        "version": "0x0",
        "witnesses": [
          "0x650000000c00000055000000490000001000000030000000310000001892ea40d82b53c678ff88312450bbb17e164d7a3e0a90941aa58839f56f8df20114000000b2e61ff569acf041b3c2c17724e2379c581eeac30c00000054455354206d657373616765"
        ]
      },
      "hash": "0xbaf7e4db2fd002f19a597ca1a31dfe8cfe26ed8cebc91f52b75b16a7a5ec8bab"
    },
    "compact_target": "0x1e083126",
    "current_time": "0x174c45e17a3",
    "cycles_limit": "0xd09dc300",
    "dao": "0xd495a106684401001e47c0ae1d5930009449d26e32380000000721efd0030000",
    "epoch": "0x7080019000001",
    "extension": null,
    "number": "0x401",
    "parent_hash": "0xa5f5c85987a15de25661e5a214f2c1449cd803f071acc7999820f25246471f40",
    "proposals": ["0xa0ef4eb5f4ceeb08a4c8"],
    "transactions": [],
    "uncles": [
      {
        "hash": "0xdca341a42890536551f99357612cef7148ed471e3b6419d0844a4e400be6ee94",
        "header": {
          "compact_target": "0x1e083126",
          "dao": "0xb5a3e047474401001bc476b9ee573000c0c387962a38000000febffacf030000",
          "epoch": "0x7080018000001",
          "extra_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
          "nonce": "0x0",
          "number": "0x400",
          "parent_hash": "0xae003585fa15309b30b31aed3dcf385e9472c3c3e93746a6c4540629a6a1ed2d",
          "proposals_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
          "timestamp": "0x5cd2b118",
          "transactions_root": "0xc47d5b78b3c4c4c853e2a32810818940d0ee403423bea9ec7b8e566d9595206c",
          "version":"0x0"
        },
        "proposals": [],
        "required": false
      }
    ],
    "uncles_count_limit": "0x2",
    "version": "0x0",
    "work_id": "0x0"
   }
 ]
}
```


Response


```
{
  "id": 42,
  "jsonrpc": "2.0",
  "result": "0x899541646ae412a99fdbefc081e1a782605a7815998a096af16e51d4df352c75",
  "error": null
}
```


#### Method `calculate_dao_field`
* `calculate_dao_field(block_template)`
    * `block_template`: [`BlockTemplate`](#type-blocktemplate)
* result: [`Byte32`](#type-byte32)

Return calculated dao field according to specified block template.

###### Params

*   `block_template` - specified block template

###### Examples

Request


```
{
  "id": 42,
  "jsonrpc": "2.0",
  "method": "calculate_dao_field",
  "params": [
   {
    "bytes_limit": "0x91c08",
    "cellbase": {
      "cycles": null,
      "data": {
        "cell_deps": [],
        "header_deps": [],
        "inputs": [
          {
            "previous_output": {
              "index": "0xffffffff",
              "tx_hash": "0x0000000000000000000000000000000000000000000000000000000000000000"
            },
            "since": "0x401"
          }
        ],
       "outputs": [
         {
           "capacity": "0x18e64efc04",
            "lock": {
              "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
              "hash_type": "data",
              "args": "0x"
            },
            "type": null
          }
        ],
        "outputs_data": [
          "0x"
        ],
        "version": "0x0",
        "witnesses": [
          "0x650000000c00000055000000490000001000000030000000310000001892ea40d82b53c678ff88312450bbb17e164d7a3e0a90941aa58839f56f8df20114000000b2e61ff569acf041b3c2c17724e2379c581eeac30c00000054455354206d657373616765"
        ]
      },
      "hash": "0xbaf7e4db2fd002f19a597ca1a31dfe8cfe26ed8cebc91f52b75b16a7a5ec8bab"
    },
    "compact_target": "0x1e083126",
    "current_time": "0x174c45e17a3",
    "cycles_limit": "0xd09dc300",
    "dao": "0xd495a106684401001e47c0ae1d5930009449d26e32380000000721efd0030000",
    "epoch": "0x7080019000001",
    "extension": null,
    "number": "0x401",
    "parent_hash": "0xa5f5c85987a15de25661e5a214f2c1449cd803f071acc7999820f25246471f40",
    "proposals": ["0xa0ef4eb5f4ceeb08a4c8"],
    "transactions": [],
    "uncles": [
      {
        "hash": "0xdca341a42890536551f99357612cef7148ed471e3b6419d0844a4e400be6ee94",
        "header": {
          "compact_target": "0x1e083126",
          "dao": "0xb5a3e047474401001bc476b9ee573000c0c387962a38000000febffacf030000",
          "epoch": "0x7080018000001",
          "extra_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
          "nonce": "0x0",
          "number": "0x400",
          "parent_hash": "0xae003585fa15309b30b31aed3dcf385e9472c3c3e93746a6c4540629a6a1ed2d",
          "proposals_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
          "timestamp": "0x5cd2b118",
          "transactions_root": "0xc47d5b78b3c4c4c853e2a32810818940d0ee403423bea9ec7b8e566d9595206c",
          "version":"0x0"
        },
        "proposals": [],
        "required": false
      }
    ],
    "uncles_count_limit": "0x2",
    "version": "0x0",
    "work_id": "0x0"
   }
  ]
}
```


Response


```
{
  "id": 42,
  "jsonrpc": "2.0",
  "result": "0xd495a106684401001e47c0ae1d5930009449d26e32380000000721efd0030000",
  "error": null
}
```


### Module Miner

RPC Module Miner for miners.

A miner gets a template from CKB, optionally selects transactions, resolves the PoW puzzle, and submits the found new block.

#### Method `get_block_template`
* `get_block_template(bytes_limit, proposals_limit, max_version)`
    * `bytes_limit`: [`Uint64`](#type-uint64) `|` `null`
    * `proposals_limit`: [`Uint64`](#type-uint64) `|` `null`
    * `max_version`: [`Version`](#type-version) `|` `null`
* result: [`BlockTemplate`](#type-blocktemplate)

Returns block template for miners.

Miners can assemble the new block from the template. The RPC is designed to allow miners to remove transactions and adding new transactions to the block.

###### Params

*   `bytes_limit` - the max serialization size in bytes of the block. (**Optional:** the default is the consensus limit.)

*   `proposals_limit` - the max count of proposals. (**Optional:** the default is the consensus limit.)

*   `max_version` - the max block version. (**Optional:** the default is one configured in the current client version.)

###### Examples

Request


```
{
  "id": 42,
  "jsonrpc": "2.0",
  "method": "get_block_template",
  "params": [
    null,
    null,
    null
  ]
}
```


Response


```
{
  "id": 42,
  "jsonrpc": "2.0",
  "result": {
    "bytes_limit": "0x91c08",
    "cellbase": {
      "cycles": null,
      "data": {
        "cell_deps": [],
        "header_deps": [],
        "inputs": [
          {
            "previous_output": {
              "index": "0xffffffff",
              "tx_hash": "0x0000000000000000000000000000000000000000000000000000000000000000"
            },
            "since": "0x401"
          }
        ],
        "outputs": [
          {
            "capacity": "0x18e64efc04",
            "lock": {
              "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
              "hash_type": "data",
              "args": "0x"
            },
            "type": null
          }
        ],
        "outputs_data": [
          "0x"
        ],
        "version": "0x0",
        "witnesses": [
          "0x6a0000000c00000055000000490000001000000030000000310000001892ea40d82b53c678ff88312450bbb17e164d7a3e0a90941aa58839f56f8df20114000000b2e61ff569acf041b3c2c17724e2379c581eeac311000000000000002054455354206d657373616765"
        ]
      },
      "hash": "0xbaf7e4db2fd002f19a597ca1a31dfe8cfe26ed8cebc91f52b75b16a7a5ec8bab"
    },
    "compact_target": "0x1e083126",
    "current_time": "0x174c45e17a3",
    "cycles_limit": "0xd09dc300",
    "dao": "0xd495a106684401001e47c0ae1d5930009449d26e32380000000721efd0030000",
    "epoch": "0x7080019000001",
    "extension": null,
    "number": "0x401",
    "parent_hash": "0xa5f5c85987a15de25661e5a214f2c1449cd803f071acc7999820f25246471f40",
    "proposals": ["0xa0ef4eb5f4ceeb08a4c8"],
    "transactions": [],
    "uncles": [
      {
        "hash": "0xdca341a42890536551f99357612cef7148ed471e3b6419d0844a4e400be6ee94",
        "header": {
          "compact_target": "0x1e083126",
          "dao": "0xb5a3e047474401001bc476b9ee573000c0c387962a38000000febffacf030000",
          "epoch": "0x7080018000001",
          "extra_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
          "nonce": "0x0",
          "number": "0x400",
          "parent_hash": "0xae003585fa15309b30b31aed3dcf385e9472c3c3e93746a6c4540629a6a1ed2d",
          "proposals_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
          "timestamp": "0x5cd2b118",
          "transactions_root": "0xc47d5b78b3c4c4c853e2a32810818940d0ee403423bea9ec7b8e566d9595206c",
          "version":"0x0"
        },
        "proposals": [],
        "required": false
      }
    ],
    "uncles_count_limit": "0x2",
    "version": "0x0",
    "work_id": "0x0"
  }
}
```


#### Method `submit_block`
* `submit_block(work_id, block)`
    * `work_id`: `string`
    * `block`: [`Block`](#type-block)
* result: [`H256`](#type-h256)

Submit new block to the network.

###### Params

*   `work_id` - The same work ID returned from [`get_block_template`](#method-get_block_template).

*   `block` - The assembled block from the block template and which PoW puzzle has been resolved.

###### Examples

Request


```
{
  "id": 42,
  "jsonrpc": "2.0",
  "method": "submit_block",
  "params": [
    "example",
    {
      "header": {
        "compact_target": "0x1e083126",
        "dao": "0xb5a3e047474401001bc476b9ee573000c0c387962a38000000febffacf030000",
        "epoch": "0x7080018000001",
        "extra_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
        "nonce": "0x0",
        "number": "0x400",
        "parent_hash": "0xae003585fa15309b30b31aed3dcf385e9472c3c3e93746a6c4540629a6a1ed2d",
        "proposals_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
        "timestamp": "0x5cd2b117",
        "transactions_root": "0xc47d5b78b3c4c4c853e2a32810818940d0ee403423bea9ec7b8e566d9595206c",
        "version": "0x0"
      },
      "proposals": [],
      "transactions": [
        {
          "cell_deps": [],
          "header_deps": [],
          "inputs": [
            {
              "previous_output": {
                "index": "0xffffffff",
                "tx_hash": "0x0000000000000000000000000000000000000000000000000000000000000000"
              },
              "since": "0x400"
            }
          ],
          "outputs": [
            {
              "capacity": "0x18e64b61cf",
              "lock": {
                "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
                "hash_type": "data",
                "args": "0x"
              },
              "type": null
            }
          ],
          "outputs_data": [
            "0x"
          ],
          "version": "0x0",
          "witnesses": [
            "0x450000000c000000410000003500000010000000300000003100000028e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5000000000000000000"
          ]
        }
      ],
      "uncles": []
    }
  ]
}
```


Response


```
{
  "id": 42,
  "jsonrpc": "2.0",
  "result": "0xa5f5c85987a15de25661e5a214f2c1449cd803f071acc7999820f25246471f40"
}
```


### Module Net

RPC Module Net for P2P network.

#### Method `local_node_info`
* `local_node_info()`
* result: [`LocalNode`](#type-localnode)

Returns the local node information.

The local node means the node itself which is serving the RPC.

###### Examples

Request


```
{
  "id": 42,
  "jsonrpc": "2.0",
  "method": "local_node_info",
  "params": []
}
```


Response


```
{
  "id": 42,
  "jsonrpc": "2.0",
  "result": {
    "active": true,
    "addresses": [
      {
        "address": "/ip4/192.168.0.2/tcp/8112/p2p/QmTRHCdrRtgUzYLNCin69zEvPvLYdxUZLLfLYyHVY3DZAS",
        "score": "0xff"
      },
      {
        "address": "/ip4/0.0.0.0/tcp/8112/p2p/QmTRHCdrRtgUzYLNCin69zEvPvLYdxUZLLfLYyHVY3DZAS",
        "score": "0x1"
      }
    ],
    "connections": "0xb",
    "node_id": "QmTRHCdrRtgUzYLNCin69zEvPvLYdxUZLLfLYyHVY3DZAS",
    "protocols": [
      {
        "id": "0x0",
        "name": "/ckb/ping",
        "support_versions": [
          "0.0.1"
        ]
      },
      {
        "id": "0x1",
        "name": "/ckb/discovery",
        "support_versions": [
          "0.0.1"
        ]
      }
    ],
    "version": "0.34.0 (f37f598 2020-07-17)"
  }
}
```


#### Method `get_peers`
* `get_peers()`
* result: `Array<` [`RemoteNode`](#type-remotenode) `>`

Returns the connected peers’ information.

###### Examples

Request


```
{
  "id": 42,
  "jsonrpc": "2.0",
  "method": "get_peers",
  "params": []
}
```


Response


```
{
  "id": 42,
  "jsonrpc": "2.0",
  "result": [
    {
      "addresses": [
        {
          "address": "/ip6/::ffff:18.185.102.19/tcp/8115/p2p/QmXwUgF48ULy6hkgfqrEwEfuHW7WyWyWauueRDAYQHNDfN",
          "score": "0x64"
        },
        {
          "address": "/ip4/18.185.102.19/tcp/8115/p2p/QmXwUgF48ULy6hkgfqrEwEfuHW7WyWyWauueRDAYQHNDfN",
          "score": "0x64"
        }
      ],
      "connected_duration": "0x2f",
      "is_outbound": true,
      "last_ping_duration": "0x1a",
      "node_id": "QmXwUgF48ULy6hkgfqrEwEfuHW7WyWyWauueRDAYQHNDfN",
      "protocols": [
        {
          "id": "0x4",
          "version": "0.0.1"
        },
        {
          "id": "0x2",
          "version": "0.0.1"
        },
        {
          "id": "0x1",
          "version": "0.0.1"
        },
        {
          "id": "0x64",
          "version": "1"
        },
        {
          "id": "0x6e",
          "version": "1"
        },
        {
          "id": "0x66",
          "version": "1"
        },
        {
          "id": "0x65",
          "version": "1"
        },
        {
          "id": "0x0",
          "version": "0.0.1"
        }
      ],
      "sync_state": {
        "best_known_header_hash": null,
        "best_known_header_number": null,
        "can_fetch_count": "0x80",
        "inflight_count": "0xa",
        "last_common_header_hash": null,
        "last_common_header_number": null,
        "unknown_header_list_size": "0x20"
      },
      "version": "0.34.0 (f37f598 2020-07-17)"
    },
    {
      "addresses": [
        {
          "address": "/ip4/174.80.182.60/tcp/52965/p2p/QmVTMd7SEXfxS5p4EEM5ykTe1DwWWVewEM3NwjLY242vr2",
          "score": "0x1"
        }
      ],
      "connected_duration": "0x95",
      "is_outbound": true,
      "last_ping_duration": "0x41",
      "node_id": "QmSrkzhdBMmfCGx8tQGwgXxzBg8kLtX8qMcqECMuKWsxDV",
      "protocols": [
        {
          "id": "0x0",
          "version": "0.0.1"
        },
        {
          "id": "0x2",
          "version": "0.0.1"
        },
        {
          "id": "0x6e",
          "version": "1"
        },
        {
          "id": "0x66",
          "version": "1"
        },
        {
          "id": "0x1",
          "version": "0.0.1"
        },
        {
          "id": "0x65",
          "version": "1"
        },
        {
          "id": "0x64",
          "version": "1"
        },
        {
          "id": "0x4",
          "version": "0.0.1"
        }
      ],
      "sync_state": {
        "best_known_header_hash": "0x2157c72b3eddd41a7a14c361173cd22ef27d7e0a29eda2e511ee0b3598c0b895",
        "best_known_header_number": "0xdb835",
        "can_fetch_count": "0x80",
        "inflight_count": "0xa",
        "last_common_header_hash": "0xc63026bd881d880bb142c855dc8153187543245f0a94391c831c75df31f263c4",
        "last_common_header_number": "0x4dc08",
        "unknown_header_list_size": "0x1f"
      },
      "version": "0.30.1 (5cc1b75 2020-03-23)"
    }
  ]
}
```


#### Method `get_banned_addresses`
* `get_banned_addresses()`
* result: `Array<` [`BannedAddr`](#type-bannedaddr) `>`

Returns all banned IPs/Subnets.

###### Examples

Request


```
{
  "id": 42,
  "jsonrpc": "2.0",
  "method": "get_banned_addresses",
  "params": []
}
```


Response


```
{
  "id": 42,
  "jsonrpc": "2.0",
  "result": [
    {
      "address": "192.168.0.2/32",
      "ban_reason": "",
      "ban_until": "0x1ac89236180",
      "created_at": "0x16bde533338"
    }
  ]
}
```


#### Method `clear_banned_addresses`
* `clear_banned_addresses()`
* result: `null`

Clears all banned IPs/Subnets.

###### Examples

Request


```
{
  "id": 42,
  "jsonrpc": "2.0",
  "method": "clear_banned_addresses",
  "params": []
}
```


Response


```
{
  "id": 42,
  "jsonrpc": "2.0",
  "result": null
}
```


#### Method `set_ban`
* `set_ban(address, command, ban_time, absolute, reason)`
    * `address`: `string`
    * `command`: `string`
    * `ban_time`: [`Timestamp`](#type-timestamp) `|` `null`
    * `absolute`: `boolean` `|` `null`
    * `reason`: `string` `|` `null`
* result: `null`

Inserts or deletes an IP/Subnet from the banned list

###### Params

*   `address` - The IP/Subnet with an optional netmask (default is /32 = single IP). Examples:
    *   “192.168.0.2” bans a single IP

    *   “192.168.0.0/24” bans IP from “192.168.0.0” to “192.168.0.255”.


*   `command` - `insert` to insert an IP/Subnet to the list, `delete` to delete an IP/Subnet from the list.

*   `ban_time` - Time in milliseconds how long (or until when if [absolute] is set) the IP is banned, optional parameter, null means using the default time of 24h

*   `absolute` - If set, the `ban_time` must be an absolute timestamp in milliseconds since epoch, optional parameter.

*   `reason` - Ban reason, optional parameter.

###### Errors

*   [`InvalidParams (-32602)`](#error-invalidparams)
    *   Expected `address` to be a valid IP address with an optional netmask.

    *   Expected `command` to be in the list [insert, delete].


###### Examples

Request


```
{
  "id": 42,
  "jsonrpc": "2.0",
  "method": "set_ban",
  "params": [
    "192.168.0.2",
    "insert",
    "0x1ac89236180",
    true,
    "set_ban example"
  ]
}
```


Response


```
{
  "id": 42,
  "jsonrpc": "2.0",
  "result": null
}
```


#### Method `sync_state`
* `sync_state()`
* result: [`SyncState`](#type-syncstate)

Returns chain synchronization state of this node.

###### Examples

Request


```
{
  "id": 42,
  "jsonrpc": "2.0",
  "method": "sync_state",
  "params": []
}
```


Response


```
{
  "id": 42,
  "jsonrpc": "2.0",
  "result": {
    "best_known_block_number": "0x400",
    "best_known_block_timestamp": "0x5cd2b117",
    "fast_time": "0x3e8",
    "ibd": true,
    "inflight_blocks_count": "0x0",
    "low_time": "0x5dc",
    "normal_time": "0x4e2",
    "orphan_blocks_count": "0x0"
  }
}
```


#### Method `set_network_active`
* `set_network_active(state)`
    * `state`: `boolean`
* result: `null`

Disable/enable all p2p network activity

###### Params

*   `state` - true to enable networking, false to disable

###### Examples

Request


```
{
  "id": 42,
  "jsonrpc": "2.0",
  "method": "set_network_active",
  "params": [
    false
  ]
}
```


Response


```
{
  "id": 42,
  "jsonrpc": "2.0",
  "result": null
}
```


#### Method `add_node`
* `add_node(peer_id, address)`
    * `peer_id`: `string`
    * `address`: `string`
* result: `null`

Attempts to add a node to the peers list and try connecting to it.

###### Params

*   `peer_id` - The node id of the node.

*   `address` - The address of the node.

The full P2P address is usually displayed as `address/peer_id`, for example in the log


```
2020-09-16 15:31:35.191 +08:00 NetworkRuntime INFO ckb_network::network
  Listen on address: /ip4/192.168.2.100/tcp/8114/QmUsZHPbjjzU627UZFt4k8j6ycEcNvXRnVGxCPKqwbAfQS
```


And in RPC `local_node_info`:


```
{
  "addresses": [
    {
      "address": "/ip4/192.168.2.100/tcp/8114/QmUsZHPbjjzU627UZFt4k8j6ycEcNvXRnVGxCPKqwbAfQS",
      "score": "0xff"
    }
  ]
}
```


In both of these examples,

*   `peer_id` is `QmUsZHPbjjzU627UZFt4k8j6ycEcNvXRnVGxCPKqwbAfQS`,

*   and `address` is `/ip4/192.168.2.100/tcp/8114`

###### Examples

Request


```
{
  "id": 42,
  "jsonrpc": "2.0",
  "method": "add_node",
  "params": [
    "QmUsZHPbjjzU627UZFt4k8j6ycEcNvXRnVGxCPKqwbAfQS",
    "/ip4/192.168.2.100/tcp/8114"
  ]
}
```


Response


```
{
  "id": 42,
  "jsonrpc": "2.0",
  "result": null
}
```


#### Method `remove_node`
* `remove_node(peer_id)`
    * `peer_id`: `string`
* result: `null`

Attempts to remove a node from the peers list and try disconnecting from it.

###### Params

*   `peer_id` - The peer id of the node.

This is the last part of a full P2P address. For example, in address “/ip4/192.168.2.100/tcp/8114/QmUsZHPbjjzU627UZFt4k8j6ycEcNvXRnVGxCPKqwbAfQS”, the `peer_id` is `QmUsZHPbjjzU627UZFt4k8j6ycEcNvXRnVGxCPKqwbAfQS`.

###### Examples

Request


```
{
  "id": 42,
  "jsonrpc": "2.0",
  "method": "remove_node",
  "params": [
    "QmUsZHPbjjzU627UZFt4k8j6ycEcNvXRnVGxCPKqwbAfQS"
  ]
}
```


Response


```
{
  "id": 42,
  "jsonrpc": "2.0",
  "result": null
}
```


#### Method `ping_peers`
* `ping_peers()`
* result: `null`

Requests that a ping is sent to all connected peers, to measure ping time.

###### Examples

Requests


```
{
  "id": 42,
  "jsonrpc": "2.0",
  "method": "ping_peers",
  "params": []
}
```


Response


```
{
  "id": 42,
  "jsonrpc": "2.0",
  "result": null
}
```


### Module Pool

RPC Module Pool for transaction memory pool.

#### Method `send_transaction`
* `send_transaction(tx, outputs_validator)`
    * `tx`: [`Transaction`](#type-transaction)
    * `outputs_validator`: [`OutputsValidator`](#type-outputsvalidator) `|` `null`
* result: [`H256`](#type-h256)

Submits a new transaction into the transaction pool. If the transaction is already in the pool, rebroadcast it to peers.

###### Params

*   `transaction` - The transaction.

*   `outputs_validator` - Validates the transaction outputs before entering the tx-pool. (**Optional**, default is “well_known_scripts_only”).

###### Errors

*   [`PoolRejectedTransactionByOutputsValidator (-1102)`](#error-poolrejectedtransactionbyoutputsvalidator) - The transaction is rejected by the validator specified by `outputs_validator`. If you really want to send transactions with advanced scripts, please set `outputs_validator` to “passthrough”.

*   [`PoolRejectedTransactionByMinFeeRate (-1104)`](#error-poolrejectedtransactionbyminfeerate) - The transaction fee rate must be greater than or equal to the config option `tx_pool.min_fee_rate`.

*   [`PoolRejectedTransactionByMaxAncestorsCountLimit (-1105)`](#error-poolrejectedtransactionbymaxancestorscountlimit) - The ancestors count must be greater than or equal to the config option `tx_pool.max_ancestors_count`.

*   [`PoolIsFull (-1106)`](#error-poolisfull) - Pool is full.

*   [`PoolRejectedDuplicatedTransaction (-1107)`](#error-poolrejectedduplicatedtransaction) - The transaction is already in the pool.

*   [`TransactionFailedToResolve (-301)`](#error-transactionfailedtoresolve) - Failed to resolve the referenced cells and headers used in the transaction, as inputs or dependencies.

*   [`TransactionFailedToVerify (-302)`](#error-transactionfailedtoverify) - Failed to verify the transaction.

###### Examples

Request


```
{
  "id": 42,
  "jsonrpc": "2.0",
  "method": "send_transaction",
  "params": [
    {
      "cell_deps": [
        {
          "dep_type": "code",
          "out_point": {
            "index": "0x0",
            "tx_hash": "0xa4037a893eb48e18ed4ef61034ce26eba9c585f15c9cee102ae58505565eccc3"
          }
        }
      ],
      "header_deps": [
        "0x7978ec7ce5b507cfb52e149e36b1a23f6062ed150503c85bbf825da3599095ed"
      ],
      "inputs": [
        {
          "previous_output": {
            "index": "0x0",
            "tx_hash": "0x365698b50ca0da75dca2c87f9e7b563811d3b5813736b8cc62cc3b106faceb17"
          },
          "since": "0x0"
        }
      ],
      "outputs": [
        {
          "capacity": "0x2540be400",
          "lock": {
            "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
            "hash_type": "data",
            "args": "0x"
          },
          "type": null
        }
      ],
      "outputs_data": [
        "0x"
      ],
      "version": "0x0",
      "witnesses": []
    },
    "passthrough"
  ]
}
```


Response


```
{
  "id": 42,
  "jsonrpc": "2.0",
  "result": "0xa0ef4eb5f4ceeb08a4c8524d84c5da95dce2f608e0ca2ec8091191b0f330c6e3"
}
```


#### Method `remove_transaction`
* `remove_transaction(tx_hash)`
    * `tx_hash`: [`H256`](#type-h256)
* result: `boolean`

Removes a transaction and all transactions which depends on it from tx pool if it exists.

###### Params

*   `tx_hash` - Hash of a transaction.

###### Returns

If the transaction exists, return true; otherwise, return false.

###### Examples

Request


```
{
  "id": 42,
  "jsonrpc": "2.0",
  "method": "remove_transaction",
  "params": [
    "0xa0ef4eb5f4ceeb08a4c8524d84c5da95dce2f608e0ca2ec8091191b0f330c6e3"
  ]
}
```


Response


```
{
  "id": 42,
  "jsonrpc": "2.0",
  "result": true
}
```


#### Method `tx_pool_info`
* `tx_pool_info()`
* result: [`TxPoolInfo`](#type-txpoolinfo)

Returns the transaction pool information.

###### Examples

Request


```
{
  "id": 42,
  "jsonrpc": "2.0",
  "method": "tx_pool_info",
  "params": []
}
```


Response


```
{
  "id": 42,
  "jsonrpc": "2.0",
  "result": {
    "last_txs_updated_at": "0x0",
    "min_fee_rate": "0x0",
    "orphan": "0x0",
    "pending": "0x1",
    "proposed": "0x0",
    "tip_hash": "0xa5f5c85987a15de25661e5a214f2c1449cd803f071acc7999820f25246471f40",
    "tip_number": "0x400",
    "total_tx_cycles": "0x219",
    "total_tx_size": "0x112"
  }
}
```


#### Method `clear_tx_pool`
* `clear_tx_pool()`
* result: `null`

Removes all transactions from the transaction pool.

###### Examples

Request


```
{
  "id": 42,
  "jsonrpc": "2.0",
  "method": "clear_tx_pool",
  "params": []
}
```


Response


```
{
  "id": 42,
  "jsonrpc": "2.0",
  "result": null
}
```


#### Method `get_raw_tx_pool`
* `get_raw_tx_pool(verbose)`
    * `verbose`: `boolean` `|` `null`
* result: [`RawTxPool`](#type-rawtxpool)

Returns all transaction ids in tx pool as a json array of string transaction ids.

###### Params

*   `verbose` - True for a json object, false for array of transaction ids, default=false

###### Examples

Request


```
{
  "id": 42,
  "jsonrpc": "2.0",
  "method": "get_raw_tx_pool",
  "params": [true]
}
```


Response


```
{
  "id": 42,
  "jsonrpc": "2.0",
  "result":
   {
       "pending": {
           "0xa0ef4eb5f4ceeb08a4c8524d84c5da95dce2f608e0ca2ec8091191b0f330c6e3": {
               "cycles": "0x219",
               "size": "0x112",
               "fee": "0x16923f7dcf",
               "ancestors_size": "0x112",
               "ancestors_cycles": "0x219",
               "ancestors_count": "0x1",
               "timestamp": "0x17c983e6e44"
           }
       },
       "proposed": {}
   }
}
```


#### Method `tx_pool_ready`
* `tx_pool_ready()`
* result: `boolean`

Returns whether tx-pool service is started, ready for request.

###### Examples

Request


```
{
  "id": 42,
  "jsonrpc": "2.0",
  "method": "tx_pool_ready",
  "params": []
}
```


Response


```
{
  "id": 42,
  "jsonrpc": "2.0",
  "result": true
}
```


### Module Stats

RPC Module Stats for getting various statistic data.

#### Method `get_blockchain_info`
* `get_blockchain_info()`
* result: [`ChainInfo`](#type-chaininfo)

Returns statistics about the chain.

###### Examples

Request


```
{
  "id": 42,
  "jsonrpc": "2.0",
  "method": "get_blockchain_info",
  "params": []
}
```


Response


```
{
  "id": 42,
  "jsonrpc": "2.0",
  "result": {
    "alerts": [
      {
        "id": "0x2a",
        "message": "An example alert message!",
        "notice_until": "0x24bcca57c00",
        "priority": "0x1"
      }
    ],
    "chain": "ckb",
    "difficulty": "0x1f4003",
    "epoch": "0x7080018000001",
    "is_initial_block_download": true,
    "median_time": "0x5cd2b105"
  }
}
```


#### Method `get_deployments_info`
* `get_deployments_info()`
* result: [`DeploymentsInfo`](#type-deploymentsinfo)

Returns statistics about the chain.

###### Examples

Request


```
{
  "id": 42,
  "jsonrpc": "2.0",
  "method": "get_deployments_info",
  "params": []
}
```


Response


```
{
  "id": 42,
  "jsonrpc": "2.0",
  "result": {
    "epoch": "0x1",
    "hash": "0xa5f5c85987a15de25661e5a214f2c1449cd803f071acc7999820f25246471f40",
       "deployments": {
           "Testdummy": {
               "bit": 1,
               "min_activation_epoch": "0x0",
               "start": "0x0",
               "state": "Failed",
               "timeout": "0x0"
           }
       }
  }
}
```


### Module Subscription

RPC Module Subscription that CKB node will push new messages to subscribers.

RPC subscriptions require a full duplex connection. CKB offers such connections in the form of TCP (enable with rpc.tcp_listen_address configuration option) and WebSocket (enable with rpc.ws_listen_address).

##### Examples

TCP RPC subscription:


```
telnet localhost 18114
> {"id": 2, "jsonrpc": "2.0", "method": "subscribe", "params": ["new_tip_header"]}
< {"jsonrpc":"2.0","result":0,"id":2}
< {"jsonrpc":"2.0","method":"subscribe","params":{"result":"...block header json...",
"subscription":0}}
< {"jsonrpc":"2.0","method":"subscribe","params":{"result":"...block header json...",
"subscription":0}}
< ...
> {"id": 2, "jsonrpc": "2.0", "method": "unsubscribe", "params": [0]}
< {"jsonrpc":"2.0","result":true,"id":2}
```


WebSocket RPC subscription:


```
let socket = new WebSocket("ws://localhost:28114")

socket.onmessage = function(event) {
  console.log(`Data received from server: ${event.data}`);
}

socket.send(`{"id": 2, "jsonrpc": "2.0", "method": "subscribe", "params": ["new_tip_header"]}`)

socket.send(`{"id": 2, "jsonrpc": "2.0", "method": "unsubscribe", "params": [0]}`)
```


#### Method `subscribe`
* `subscribe(topic)`
    * `topic`: `string`
* result: `string`

Subscribes to a topic.

###### Params

*   `topic` - Subscription topic (enum: new_tip_header | new_tip_block | new_transaction | proposed_transaction | rejected_transaction)

###### Returns

This RPC returns the subscription ID as the result. CKB node will push messages in the subscribed topics to the current RPC connection. The subscript ID is also attached as `params.subscription` in the push messages.

Example push message:


```
{
  "jsonrpc": "2.0",
  "method": "subscribe",
  "params": {
    "result": { ... },
    "subscription": "0x2a"
  }
}
```


###### Topics

###### `new_tip_header`

Whenever there’s a block that is appended to the canonical chain, the CKB node will publish the block header to subscribers.

The type of the `params.result` in the push message is [`HeaderView`](#type-headerview).

###### `new_tip_block`

Whenever there’s a block that is appended to the canonical chain, the CKB node will publish the whole block to subscribers.

The type of the `params.result` in the push message is [`BlockView`](#type-blockview).

###### `new_transaction`

Subscribers will get notified when a new transaction is submitted to the pool.

The type of the `params.result` in the push message is [`PoolTransactionEntry`](#type-pooltransactionentry).

###### `proposed_transaction`

Subscribers will get notified when an in-pool transaction is proposed by chain.

The type of the `params.result` in the push message is [`PoolTransactionEntry`](#type-pooltransactionentry).

###### `rejected_transaction`

Subscribers will get notified when a pending transaction is rejected by tx-pool.

The type of the `params.result` in the push message is an array contain:

The type of the `params.result` in the push message is a two-elements array, where

*   the first item type is [`PoolTransactionEntry`](#type-pooltransactionentry), and

*   the second item type is [`PoolTransactionReject`](#type-pooltransactionreject).

###### Examples

Request


```
{
  "id": 42,
  "jsonrpc": "2.0",
  "method": "subscribe",
  "params": [
    "new_tip_header"
  ]
}
```


Response


```
{
  "id": 42,
  "jsonrpc": "2.0",
  "result": "0x2a"
}
```


#### Method `unsubscribe`
* `unsubscribe(id)`
    * `id`: `string`
* result: `boolean`

Unsubscribes from a subscribed topic.

###### Params

*   `id` - Subscription ID

###### Examples

Request


```
{
  "id": 42,
  "jsonrpc": "2.0",
  "method": "unsubscribe",
  "params": [
    "0x2a"
  ]
}
```


Response


```
{
  "id": 42,
  "jsonrpc": "2.0",
  "result": true
}
```



## RPC Errors

CKB RPC error codes.

CKB RPC follows the JSON RPC specification about the [error object](https://www.jsonrpc.org/specification#error_object).

Besides the pre-defined errors, all CKB defined errors are listed here.

Here is a reference to the pre-defined errors:


|  code | message | meaning |
| --- |--- |--- |
|  -32700 | Parse error | Invalid JSON was received by the server. |
|  -32600 | Invalid Request | The JSON sent is not a valid Request object. |
|  -32601 | Method not found | The method does not exist / is not available. |
|  -32602 | Invalid params | Invalid method parameter(s). |
|  -32603 | Internal error | Internal JSON-RPC error. |
|  -32000 to -32099 | Server error | Reserved for implementation-defined server-errors. |


CKB application-defined errors follow some patterns to assign the codes:

*   -1 ~ -999 are general errors

*   -1000 ~ -2999 are module-specific errors. Each module generally gets 100 reserved error codes.

Unless otherwise noted, all the errors return optional detailed information as `string` in the error object `data` field.

### Error `CKBInternalError`

(-1): CKB internal errors are considered to never happen or only happen when the system resources are exhausted.

### Error `Deprecated`

(-2): The CKB method has been deprecated and disabled.

Set `rpc.enable_deprecated_rpc` to `true` in the config file to enable all deprecated methods.

### Error `Invalid`

(-3): Error code -3 is no longer used.

Before v0.35.0, CKB returns all RPC errors using the error code -3. CKB no longer uses -3 since v0.35.0.

### Error `RPCModuleIsDisabled`

(-4): The RPC method is not enabled.

CKB groups RPC methods into modules, and a method is enabled only when the module is explicitly enabled in the config file.

### Error `DaoError`

(-5): DAO related errors.

### Error `IntegerOverflow`

(-6): Integer operation overflow.

### Error `ConfigError`

(-7): The error is caused by a config file option.

Users have to edit the config file to fix the error.

### Error `P2PFailedToBroadcast`

(-101): The CKB local node failed to broadcast a message to its peers.

### Error `DatabaseError`

(-200): Internal database error.

The CKB node persists data to the database. This is the error from the underlying database module.

### Error `ChainIndexIsInconsistent`

(-201): The chain index is inconsistent.

An example of an inconsistent index is that the chain index says a block hash is in the chain but the block cannot be read from the database.

This is a fatal error usually due to a serious bug. Please back up the data directory and re-sync the chain from scratch.

### Error `DatabaseIsCorrupt`

(-202): The underlying database is corrupt.

This is a fatal error usually caused by the underlying database used by CKB. Please back up the data directory and re-sync the chain from scratch.

### Error `TransactionFailedToResolve`

(-301): Failed to resolve the referenced cells and headers used in the transaction, as inputs or dependencies.

### Error `TransactionFailedToVerify`

(-302): Failed to verify the transaction.

### Error `AlertFailedToVerifySignatures`

(-1000): Some signatures in the submit alert are invalid.

### Error `PoolRejectedTransactionByOutputsValidator`

(-1102): The transaction is rejected by the outputs validator specified by the RPC parameter.

### Error `PoolRejectedTransactionByIllTransactionChecker`

(-1103): Pool rejects some transactions which seem contain invalid VM instructions. See the issue link in the error message for details.

### Error `PoolRejectedTransactionByMinFeeRate`

(-1104): The transaction fee rate must be greater than or equal to the config option `tx_pool.min_fee_rate`

The fee rate is calculated as:


```
fee / (1000 * tx_serialization_size_in_block_in_bytes)
```


### Error `PoolRejectedTransactionByMaxAncestorsCountLimit`

(-1105): The in-pool ancestors count must be less than or equal to the config option `tx_pool.max_ancestors_count`

Pool rejects a large package of chained transactions to avoid certain kinds of DoS attacks.

### Error `PoolIsFull`

(-1106): The transaction is rejected because the pool has reached its limit.

### Error `PoolRejectedDuplicatedTransaction`

(-1107): The transaction is already in the pool.

### Error `PoolRejectedMalformedTransaction`

(-1108): The transaction is rejected because it does not make sense in the context.

For example, a cellbase transaction is not allowed in `send_transaction` RPC.

### Error `TransactionExpired`

(-1109): The transaction is expired from tx-pool after `expiry_hours`.

### Error `Indexer`

(-1200): The indexer error.


## RPC Types

### Type `Alert`

An alert is a message about critical problems to be broadcast to all nodes via the p2p network.

##### Examples

An example in JSON


```
{
  "id": "0x1",
  "cancel": "0x0",
  "min_version": "0.1.0",
  "max_version": "1.0.0",
  "priority": "0x1",
  "message": "An example alert message!",
  "notice_until": "0x24bcca57c00",
  "signatures": [
    "0xbd07059aa9a3d057da294c2c4d96fa1e67eeb089837c87b523f124239e18e9fc7d11bb95b720478f7f937d073517d0e4eb9a91d12da5c88a05f750362f4c214dd0",
    "0x0242ef40bb64fe3189284de91f981b17f4d740c5e24a3fc9b70059db6aa1d198a2e76da4f84ab37549880d116860976e0cf81cd039563c452412076ebffa2e4453"
  ]
}
```


#### Fields

`Alert` is a JSON object with the following fields.

*   `id`: [`AlertId`](#type-alertid) - The identifier of the alert. Clients use id to filter duplicated alerts.

*   `cancel`: [`AlertId`](#type-alertid) - Cancel a previous sent alert.

*   `min_version`: `string` `|` `null` - Optionally set the minimal version of the target clients.

    See [Semantic Version](https://semver.org/) about how to specify a version.

*   `max_version`: `string` `|` `null` - Optionally set the maximal version of the target clients.

    See [Semantic Version](https://semver.org/) about how to specify a version.

*   `priority`: [`AlertPriority`](#type-alertpriority) - Alerts are sorted by priority, highest first.

*   `notice_until`: [`Timestamp`](#type-timestamp) - The alert is expired after this timestamp.

*   `message`: `string` - Alert message.

*   `signatures`: `Array<` [`JsonBytes`](#type-jsonbytes) `>` - The list of required signatures.


### Type `AlertId`

The alert identifier that is used to filter duplicated alerts.

This is a 32-bit unsigned integer type encoded as the 0x-prefixed hex string in JSON. See examples of [Uint32](#type-uint32).

### Type `AlertMessage`

An alert sent by RPC `send_alert`.

#### Fields

`AlertMessage` is a JSON object with the following fields.

*   `id`: [`AlertId`](#type-alertid) - The unique alert ID.

*   `priority`: [`AlertPriority`](#type-alertpriority) - Alerts are sorted by priority, highest first.

*   `notice_until`: [`Timestamp`](#type-timestamp) - The alert is expired after this timestamp.

*   `message`: `string` - Alert message.


### Type `AlertPriority`

Alerts are sorted by priority. Greater integers mean higher priorities.

This is a 32-bit unsigned integer type encoded as the 0x-prefixed hex string in JSON. See examples of [Uint32](#type-uint32).

### Type `BannedAddr`

A banned P2P address.

#### Fields

`BannedAddr` is a JSON object with the following fields.

*   `address`: `string` - The P2P address.

    Example: “/ip4/192.168.0.2/tcp/8112/p2p/QmTRHCdrRtgUzYLNCin69zEvPvLYdxUZLLfLYyHVY3DZAS”

*   `ban_until`: [`Timestamp`](#type-timestamp) - The address is banned until this time.

*   `ban_reason`: `string` - The reason.

*   `created_at`: [`Timestamp`](#type-timestamp) - When this address is banned.


### Type `Block`

The JSON view of a Block used as a parameter in the RPC.

#### Fields

`Block` is a JSON object with the following fields.

*   `header`: [`Header`](#type-header) - The block header.

*   `uncles`: `Array<` [`UncleBlock`](#type-uncleblock) `>` - The uncles blocks in the block body.

*   `transactions`: `Array<` [`Transaction`](#type-transaction) `>` - The transactions in the block body.

*   `proposals`: `Array<` [`ProposalShortId`](#type-proposalshortid) `>` - The proposal IDs in the block body.


### Type `BlockEconomicState`

Block Economic State.

It includes the rewards details and when it is finalized.

#### Fields

`BlockEconomicState` is a JSON object with the following fields.

*   `issuance`: [`BlockIssuance`](#type-blockissuance) - Block base rewards.

*   `miner_reward`: [`MinerReward`](#type-minerreward) - Block rewards for miners.

*   `txs_fee`: [`Capacity`](#type-capacity) - The total fees of all transactions committed in the block.

*   `finalized_at`: [`H256`](#type-h256) - The block hash of the block which creates the rewards as cells in its cellbase transaction.


### Type `BlockIssuance`

Block base rewards.

#### Fields

`BlockIssuance` is a JSON object with the following fields.

*   `primary`: [`Capacity`](#type-capacity) - The primary base rewards.

*   `secondary`: [`Capacity`](#type-capacity) - The secondary base rewards.


### Type `BlockNumber`

Consecutive block number starting from 0.

This is a 64-bit unsigned integer type encoded as the 0x-prefixed hex string in JSON. See examples of [Uint64](#type-uint64).

### Type `BlockResponse`

The wrapper represent response of `get_block` | `get_block_by_number`, return a Block with cycles.

`BlockResponse` is equivalent to `"regular" | "with_cycles"`.

*   The block response regular format

    [`BlockView`](#type-blockview) | [`SerializedBlock`](#type-serializedblock) - The block structure

*   The block with cycles response format

    A JSON object with the following fields:

    *   `block`: [`BlockView`](#type-blockview) | [`SerializedBlock`](#type-serializedblock) - The block structure

    *   `cycles`: `Array<` [`Cycle`](#type-cycle) `>` `|` `null` - The block transactions consumed cycles.



### Type `BlockTemplate`

A block template for miners.

Miners optional pick transactions and then assemble the final block.

#### Fields

`BlockTemplate` is a JSON object with the following fields.

*   `version`: [`Version`](#type-version) - Block version.

    Miners must use it unchanged in the assembled block.

*   `compact_target`: [`Uint32`](#type-uint32) - The compacted difficulty target for the new block.

    Miners must use it unchanged in the assembled block.

*   `current_time`: [`Timestamp`](#type-timestamp) - The timestamp for the new block.

    CKB node guarantees that this timestamp is larger than the median of the previous 37 blocks.

    Miners can increase it to the current time. It is not recommended to decrease it, since it may violate the median block timestamp consensus rule.

*   `number`: [`BlockNumber`](#type-blocknumber) - The block number for the new block.

    Miners must use it unchanged in the assembled block.

*   `epoch`: [`EpochNumberWithFraction`](#type-epochnumberwithfraction) - The epoch progress information for the new block.

    Miners must use it unchanged in the assembled block.

*   `parent_hash`: [`H256`](#type-h256) - The parent block hash of the new block.

    Miners must use it unchanged in the assembled block.

*   `cycles_limit`: [`Cycle`](#type-cycle) - The cycles limit.

    Miners must keep the total cycles below this limit, otherwise, the CKB node will reject the block submission.

    It is guaranteed that the block does not exceed the limit if miners do not add new transactions to the block.

*   `bytes_limit`: [`Uint64`](#type-uint64) - The block serialized size limit.

    Miners must keep the block size below this limit, otherwise, the CKB node will reject the block submission.

    It is guaranteed that the block does not exceed the limit if miners do not add new transaction commitments.

*   `uncles_count_limit`: [`Uint64`](#type-uint64) - The uncle count limit.

    Miners must keep the uncles count below this limit, otherwise, the CKB node will reject the block submission.

*   `uncles`: `Array<` [`UncleTemplate`](#type-uncletemplate) `>` - Provided valid uncle blocks candidates for the new block.

    Miners must include the uncles marked as `required` in the assembled new block.

*   `transactions`: `Array<` [`TransactionTemplate`](#type-transactiontemplate) `>` - Provided valid transactions which can be committed in the new block.

    Miners must include the transactions marked as `required` in the assembled new block.

*   `proposals`: `Array<` [`ProposalShortId`](#type-proposalshortid) `>` - Provided proposal ids list of transactions for the new block.

*   `cellbase`: [`CellbaseTemplate`](#type-cellbasetemplate) - Provided cellbase transaction template.

    Miners must use it as the cellbase transaction without changes in the assembled block.

*   `work_id`: [`Uint64`](#type-uint64) - Work ID. The miner must submit the new assembled and resolved block using the same work ID.

*   `dao`: [`Byte32`](#type-byte32) - Reference DAO field.

    This field is only valid when miners use all and only use the provided transactions in the template. Two fields must be updated when miners want to select transactions:

    *   `S_i`, bytes 16 to 23

    *   `U_i`, bytes 24 to 31

    See RFC [Deposit and Withdraw in Nervos DAO](https://github.com/nervosnetwork/rfcs/blob/master/rfcs/0023-dao-deposit-withdraw/0023-dao-deposit-withdraw.md#calculation).

*   `extension`: [`JsonBytes`](#type-jsonbytes) `|` `null` - The extension for the new block.

    This field is optional. It’s a reserved field, please leave it blank. More details can be found in [CKB RFC 0031](https://github.com/nervosnetwork/rfcs/blob/master/rfcs/0031-variable-length-header-field/0031-variable-length-header-field.md).


### Type `BlockView`

The JSON view of a Block including header and body.

#### Fields

`BlockView` is a JSON object with the following fields.

*   `header`: [`HeaderView`](#type-headerview) - The block header.

*   `uncles`: `Array<` [`UncleBlockView`](#type-uncleblockview) `>` - The uncles blocks in the block body.

*   `transactions`: `Array<` [`TransactionView`](#type-transactionview) `>` - The transactions in the block body.

*   `proposals`: `Array<` [`ProposalShortId`](#type-proposalshortid) `>` - The proposal IDs in the block body.


### Type `Byte32`

Fixed-length 32 bytes binary encoded as a 0x-prefixed hex string in JSON.

##### Example


```
0xd495a106684401001e47c0ae1d5930009449d26e32380000000721efd0030000
```


#### Fields

`Byte32` is a JSON object with the following fields.

*   `0`: https://doc.rust-lang.org/1.61.0/std/primitive.array.html - Creates Bytes from the array.


### Type `Capacity`

The capacity of a cell is the value of the cell in Shannons. It is also the upper limit of the cell occupied storage size where every 100,000,000 Shannons give 1-byte storage.

This is a 64-bit unsigned integer type encoded as the 0x-prefixed hex string in JSON. See examples of [Uint64](#type-uint64).

### Type `CellData`

The cell data content and hash.

##### Examples


```
{
  "content": "0x7f454c460201010000000000000000000200f3000100000078000100000000004000000000000000980000000000000005000000400038000100400003000200010000000500000000000000000000000000010000000000000001000000000082000000000000008200000000000000001000000000000001459308d00573000000002e7368737472746162002e74657874000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000b000000010000000600000000000000780001000000000078000000000000000a0000000000000000000000000000000200000000000000000000000000000001000000030000000000000000000000000000000000000082000000000000001100000000000000000000000000000001000000000000000000000000000000",
  "hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5"
}
```


#### Fields

`CellData` is a JSON object with the following fields.

*   `content`: [`JsonBytes`](#type-jsonbytes) - Cell content.

*   `hash`: [`H256`](#type-h256) - Cell content hash.


### Type `CellDep`

The cell dependency of a transaction.

##### Examples


```
{
  "dep_type": "code",
  "out_point": {
    "index": "0x0",
    "tx_hash": "0xa4037a893eb48e18ed4ef61034ce26eba9c585f15c9cee102ae58505565eccc3"
  }
}
```


#### Fields

`CellDep` is a JSON object with the following fields.

*   `out_point`: [`OutPoint`](#type-outpoint) - Reference to the cell.

*   `dep_type`: [`DepType`](#type-deptype) - Dependency type.


### Type `CellInfo`

The JSON view of a cell combining the fields in cell output and cell data.

##### Examples


```
{
  "data": {
    "content": "0x7f454c460201010000000000000000000200f3000100000078000100000000004000000000000000980000000000000005000000400038000100400003000200010000000500000000000000000000000000010000000000000001000000000082000000000000008200000000000000001000000000000001459308d00573000000002e7368737472746162002e74657874000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000b000000010000000600000000000000780001000000000078000000000000000a0000000000000000000000000000000200000000000000000000000000000001000000030000000000000000000000000000000000000082000000000000001100000000000000000000000000000001000000000000000000000000000000",
    "hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5"
  },
  "output": {
    "capacity": "0x802665800",
    "lock": {
      "args": "0x",
      "code_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
      "hash_type": "data"
    },
    "type": null
  }
}
```


#### Fields

`CellInfo` is a JSON object with the following fields.

*   `output`: [`CellOutput`](#type-celloutput) - Cell fields appears in the transaction `outputs` array.

*   `data`: [`CellData`](#type-celldata) `|` `null` - Cell data.

    This is `null` when the data is not requested, which does not mean the cell data is empty.


### Type `CellInput`

The input cell of a transaction.

##### Examples


```
{
  "previous_output": {
    "index": "0x0",
    "tx_hash": "0x365698b50ca0da75dca2c87f9e7b563811d3b5813736b8cc62cc3b106faceb17"
  },
  "since": "0x0"
}
```


#### Fields

`CellInput` is a JSON object with the following fields.

*   `since`: [`Uint64`](#type-uint64) - Restrict when the transaction can be committed into the chain.

    See the RFC [Transaction valid since](https://github.com/nervosnetwork/rfcs/blob/master/rfcs/0017-tx-valid-since/0017-tx-valid-since.md).

*   `previous_output`: [`OutPoint`](#type-outpoint) - Reference to the input cell.


### Type `CellOutput`

The fields of an output cell except the cell data.

##### Examples


```
{
  "capacity": "0x2540be400",
  "lock": {
    "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
    "hash_type": "data",
    "args": "0x"
  },
  "type": null
}
```


#### Fields

`CellOutput` is a JSON object with the following fields.

*   `capacity`: [`Capacity`](#type-capacity) - The cell capacity.

    The capacity of a cell is the value of the cell in Shannons. It is also the upper limit of the cell occupied storage size where every 100,000,000 Shannons give 1-byte storage.

*   `lock`: [`Script`](#type-script) - The lock script.

*   `type_`: [`Script`](#type-script) `|` `null` - The optional type script.

    The JSON field name is “type”.


### Type `CellWithStatus`

The JSON view of a cell with its status information.

##### Examples


```
{
  "cell": {
    "data": {
      "content": "0x7f454c460201010000000000000000000200f3000100000078000100000000004000000000000000980000000000000005000000400038000100400003000200010000000500000000000000000000000000010000000000000001000000000082000000000000008200000000000000001000000000000001459308d00573000000002e7368737472746162002e74657874000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000b000000010000000600000000000000780001000000000078000000000000000a0000000000000000000000000000000200000000000000000000000000000001000000030000000000000000000000000000000000000082000000000000001100000000000000000000000000000001000000000000000000000000000000",
      "hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5"
    },
    "output": {
      "capacity": "0x802665800",
      "lock": {
        "args": "0x",
        "code_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
        "hash_type": "data"
      },
      "type": null
    }
  },
  "status": "live"
}
```



```
{
  "cell": null,
  "status": "unknown"
}
```


#### Fields

`CellWithStatus` is a JSON object with the following fields.

*   `cell`: [`CellInfo`](#type-cellinfo) `|` `null` - The cell information.

    For performance issues, CKB only keeps the information for live cells.

*   `status`: `string` - Status of the cell.

    Allowed values: “live”, “dead”, “unknown”.

    *   `live` - The transaction creating this cell is in the chain, and there are no transactions found in the chain that uses this cell as an input.

    *   `dead` - (**Deprecated**: the dead status will be removed since 0.36.0, please do not rely on the logic that differentiates dead and unknown cells.) The transaction creating this cell is in the chain, and a transaction is found in the chain which uses this cell as an input.

    *   `unknown` - CKB does not know the status of the cell. Either the transaction creating this cell is not in the chain yet, or it is no longer live.


### Type `CellbaseTemplate`

The cellbase transaction template of the new block for miners.

#### Fields

`CellbaseTemplate` is a JSON object with the following fields.

*   `hash`: [`H256`](#type-h256) - The cellbase transaction hash.

*   `cycles`: [`Cycle`](#type-cycle) `|` `null` - The hint of how many cycles this transaction consumes.

    Miners can utilize this field to ensure that the total cycles do not exceed the limit while selecting transactions.

*   `data`: [`Transaction`](#type-transaction) - The cellbase transaction.


### Type `ChainInfo`

Chain information.

#### Fields

`ChainInfo` is a JSON object with the following fields.

*   `chain`: `string` - The network name.

    Examples:

    *   “ckb” - Mirana the mainnet.

    *   “ckb_testnet” - Pudge the testnet.

*   `median_time`: [`Timestamp`](#type-timestamp) - The median time of the last 37 blocks, including the tip block.

*   `epoch`: [`EpochNumberWithFraction`](#type-epochnumberwithfraction) - The epoch information of tip block in the chain.

*   `difficulty`: [`U256`](#type-u256) - Current difficulty.

    Decoded from the epoch `compact_target`.

*   `is_initial_block_download`: `boolean` - Whether the local node is in IBD, Initial Block Download.

    When a node starts and its chain tip timestamp is far behind the wall clock, it will enter the IBD until it catches up the synchronization.

    During IBD, the local node only synchronizes the chain with one selected remote node and stops responding the most P2P requests.

*   `alerts`: `Array<` [`AlertMessage`](#type-alertmessage) `>` - Active alerts stored in the local node.


### Type `Consensus`

Consensus defines various parameters that influence chain consensus

#### Fields

`Consensus` is a JSON object with the following fields.

*   `id`: `string` - Names the network.

*   `genesis_hash`: [`H256`](#type-h256) - The genesis block hash

*   `dao_type_hash`: [`H256`](#type-h256) `|` `null` - The dao type hash

*   `secp256k1_blake160_sighash_all_type_hash`: [`H256`](#type-h256) `|` `null` - The secp256k1_blake160_sighash_all_type_hash

*   `secp256k1_blake160_multisig_all_type_hash`: [`H256`](#type-h256) `|` `null` - The secp256k1_blake160_multisig_all_type_hash

*   `initial_primary_epoch_reward`: [`Capacity`](#type-capacity) - The initial primary_epoch_reward

*   `secondary_epoch_reward`: [`Capacity`](#type-capacity) - The secondary primary_epoch_reward

*   `max_uncles_num`: [`Uint64`](#type-uint64) - The maximum amount of uncles allowed for a block

*   `orphan_rate_target`: [`RationalU256`](#type-rationalu256) - The expected orphan_rate

*   `epoch_duration_target`: [`Uint64`](#type-uint64) - The expected epoch_duration

*   `tx_proposal_window`: [`ProposalWindow`](#type-proposalwindow) - The two-step-transaction-confirmation proposal window

*   `proposer_reward_ratio`: [`RationalU256`](#type-rationalu256) - The two-step-transaction-confirmation proposer reward ratio

*   `cellbase_maturity`: [`EpochNumberWithFraction`](#type-epochnumberwithfraction) - The Cellbase maturity

*   `median_time_block_count`: [`Uint64`](#type-uint64) - This parameter indicates the count of past blocks used in the median time calculation

*   `max_block_cycles`: [`Cycle`](#type-cycle) - Maximum cycles that all the scripts in all the commit transactions can take

*   `max_block_bytes`: [`Uint64`](#type-uint64) - Maximum number of bytes to use for the entire block

*   `block_version`: [`Version`](#type-version) - The block version number supported

*   `tx_version`: [`Version`](#type-version) - The tx version number supported

*   `type_id_code_hash`: [`H256`](#type-h256) - The “TYPE_ID” in hex

*   `max_block_proposals_limit`: [`Uint64`](#type-uint64) - The Limit to the number of proposals per block

*   `primary_epoch_reward_halving_interval`: [`Uint64`](#type-uint64) - Primary reward is cut in half every halving_interval epoch

*   `permanent_difficulty_in_dummy`: `boolean` - Keep difficulty be permanent if the pow is dummy

*   `hardfork_features`: `Array<` [`HardForkFeature`](#type-hardforkfeature) `>` - Hardfork features


### Type `Cycle`

Count of cycles consumed by CKB VM to run scripts.

This is a 64-bit unsigned integer type encoded as the 0x-prefixed hex string in JSON. See examples of [Uint64](#type-uint64).

### Type `DaoWithdrawingCalculationKind`

An enum to represent the two kinds of dao withdrawal amount calculation option. `DaoWithdrawingCalculationKind` is equivalent to [`H256`](#type-h256) `|` [`OutPoint`](#type-outpoint).

`DaoWithdrawingCalculationKind` is equivalent to `"withdrawing_header_hash" | "withdrawing_out_point"`.

*   the assumed reference block hash for withdrawing phase 1 transaction
*   the out point of the withdrawing phase 1 transaction


### Type `DepType`

The dep cell type. Allowed values: “code” and “dep_group”.

`DepType` is equivalent to `"code" | "dep_group"`.

*   Type “code”.

    Use the cell itself as the dep cell.

*   Type “dep_group”.

    The cell is a dep group which members are cells. These members are used as dep cells instead of the group itself.

    The dep group stores the array of `OutPoint`s serialized via molecule in the cell data. Each `OutPoint` points to one cell member.



### Type `DeploymentInfo`

An object containing various state info regarding deployments of consensus changes

#### Fields

`DeploymentInfo` is a JSON object with the following fields.

*   `bit`: https://doc.rust-lang.org/1.61.0/std/primitive.u8.html - determines which bit in the `version` field of the block is to be used to signal the softfork lock-in and activation. It is chosen from the set {0,1,2,…,28}.

*   `start`: [`EpochNumber`](#type-epochnumber) - specifies the first epoch in which the bit gains meaning.

*   `timeout`: [`EpochNumber`](#type-epochnumber) - specifies an epoch at which the miner signaling ends. Once this epoch has been reached, if the softfork has not yet locked_in (excluding this epoch block’s bit state), the deployment is considered failed on all descendants of the block.

*   `min_activation_epoch`: [`EpochNumber`](#type-epochnumber) - specifies the epoch at which the softfork is allowed to become active.

*   `state`: [`DeploymentState`](#type-deploymentstate) - With each epoch and softfork, we associate a deployment state. The possible states are


### Type `DeploymentPos`

Deployment name

`DeploymentPos` is equivalent to `"Testdummy" | "LightClient"`.

*   Dummy
*   light client protocol


### Type `DeploymentState`

The possible softfork deployment state

`DeploymentState` is equivalent to `"Defined" | "Started" | "LockedIn" | "Active" | "Failed"`.

*   First state that each softfork starts. The 0 epoch is by definition in this state for each deployment.
*   For epochs past the `start` epoch.
*   For one epoch after the first epoch period with STARTED epochs of which at least `threshold` has the associated bit set in `version`.
*   For all epochs after the LOCKED_IN epoch.
*   For one epoch period past the `timeout_epoch`, if LOCKED_IN was not reached.


### Type `DeploymentsInfo`

Chain information.

#### Fields

`DeploymentsInfo` is a JSON object with the following fields.

*   `hash`: [`H256`](#type-h256) - requested block hash

*   `epoch`: [`EpochNumber`](#type-epochnumber) - requested block epoch

*   `deployments`: `{ [ key:` [`DeploymentPos`](#type-deploymentpos) `]: ` [`DeploymentInfo`](#type-deploymentinfo) `}` - deployments info


### Type `Either`

The enum `Either` with variants `Left` and `Right` is a general purpose sum type with two cases.

`Either` is equivalent to `"left" | "right"`.

*   A value of type `L`.
*   A value of type `R`.


### Type `EpochNumber`

Consecutive epoch number starting from 0.

This is a 64-bit unsigned integer type encoded as the 0x-prefixed hex string in JSON. See examples of [Uint64](#type-uint64).

### Type `EpochNumberWithFraction`

The epoch indicator of a block. It shows which epoch the block is in, and the elapsed epoch fraction after adding this block.

This is a 64-bit unsigned integer type encoded as the 0x-prefixed hex string in JSON. See examples of [Uint64](#type-uint64).

The lower 56 bits of the epoch field are split into 3 parts (listed in the order from higher bits to lower bits):

*   The highest 16 bits represent the epoch length

*   The next 16 bits represent the current block index in the epoch, starting from 0.

*   The lowest 24 bits represent the current epoch number.

Assume there’s a block, which number is 11555 and in epoch 50. The epoch 50 starts from block 11000 and have 1000 blocks. The epoch field for this particular block will then be 1,099,520,939,130,930, which is calculated in the following way:


```
50 | ((11555 - 11000) << 24) | (1000 << 40)
```


### Type `EpochView`

JSON view of an epoch.

CKB adjusts difficulty based on epochs.

##### Examples


```
{
  "compact_target": "0x1e083126",
  "length": "0x708",
  "number": "0x1",
  "start_number": "0x3e8"
}
```


#### Fields

`EpochView` is a JSON object with the following fields.

*   `number`: [`EpochNumber`](#type-epochnumber) - Consecutive epoch number starting from 0.

*   `start_number`: [`BlockNumber`](#type-blocknumber) - The block number of the first block in the epoch.

    It also equals the total count of blocks in all the epochs which epoch number is less than this epoch.

*   `length`: [`BlockNumber`](#type-blocknumber) - The number of blocks in this epoch.

*   `compact_target`: [`Uint32`](#type-uint32) - The difficulty target for any block in this epoch.


### Type `EstimateCycles`

Response result of the RPC method `estimate_cycles`.

#### Fields

`EstimateCycles` is a JSON object with the following fields.

*   `cycles`: [`Cycle`](#type-cycle) - The count of cycles that the VM has consumed to verify this transaction.


### Type `FeeRateStatics`

The fee_rate statistics information, includes mean and median, unit: shannons per kilo-weight

#### Fields

`FeeRateStatics` is a JSON object with the following fields.

*   `mean`: [`Uint64`](#type-uint64) - mean

*   `median`: [`Uint64`](#type-uint64) - median


### Type `H256`

The 32-byte fixed-length binary data.

The name comes from the number of bits in the data.

In JSONRPC, it is encoded as a 0x-prefixed hex string.

#### Fields

`H256` is a JSON object with the following fields.

*   `0`: https://doc.rust-lang.org/1.61.0/std/primitive.array.html - Converts `Self` to a byte slice.


### Type `HardForkFeature`

The information about one hardfork feature.

#### Fields

`HardForkFeature` is a JSON object with the following fields.

*   `rfc`: `string` - The related RFC ID.

*   `epoch_number`: [`EpochNumber`](#type-epochnumber) `|` `null` - The first epoch when the feature is enabled, `null` indicates that the RFC has never been enabled.


### Type `Header`

The block header.

Refer to RFC [CKB Block Structure](https://github.com/nervosnetwork/rfcs/blob/master/rfcs/0027-block-structure/0027-block-structure.md).

#### Fields

`Header` is a JSON object with the following fields.

*   `version`: [`Version`](#type-version) - The block version.

    It must equal to 0 now and is reserved for future upgrades.

*   `compact_target`: [`Uint32`](#type-uint32) - The block difficulty target.

    It can be converted to a 256-bit target. Miners must ensure the Eaglesong of the header is within the target.

*   `timestamp`: [`Timestamp`](#type-timestamp) - The block timestamp.

    It is a Unix timestamp in milliseconds (1 second = 1000 milliseconds).

    Miners should put the time when the block is created in the header, however, the precision is not guaranteed. A block with a higher block number may even have a smaller timestamp.

*   `number`: [`BlockNumber`](#type-blocknumber) - The consecutive block number starting from 0.

*   `epoch`: [`EpochNumberWithFraction`](#type-epochnumberwithfraction) - The epoch information of this block.

    See `EpochNumberWithFraction` for details.

*   `parent_hash`: [`H256`](#type-h256) - The header hash of the parent block.

*   `transactions_root`: [`H256`](#type-h256) - The commitment to all the transactions in the block.

    It is a hash on two Merkle Tree roots:

    *   The root of a CKB Merkle Tree, which items are the transaction hashes of all the transactions in the block.

    *   The root of a CKB Merkle Tree, but the items are the transaction witness hashes of all the transactions in the block.

*   `proposals_hash`: [`H256`](#type-h256) - The hash on `proposals` in the block body.

    It is all zeros when `proposals` is empty, or the hash on all the bytes concatenated together.

*   `extra_hash`: [`H256`](#type-h256) - The hash on `uncles` and extension in the block body.

    The uncles hash is all zeros when `uncles` is empty, or the hash on all the uncle header hashes concatenated together. The extension hash is the hash of the extension. The extra hash is the hash on uncles hash and extension hash concatenated together.

    ##### Notice

    This field is renamed from `uncles_hash` since 0.100.0. More details can be found in [CKB RFC 0031](https://github.com/nervosnetwork/rfcs/blob/master/rfcs/0031-variable-length-header-field/0031-variable-length-header-field.md).

*   `dao`: [`Byte32`](#type-byte32) - DAO fields.

    See RFC [Deposit and Withdraw in Nervos DAO](https://github.com/nervosnetwork/rfcs/blob/master/rfcs/0023-dao-deposit-withdraw/0023-dao-deposit-withdraw.md#calculation).

*   `nonce`: [`Uint128`](#type-uint128) - Miner can modify this field to find a proper value such that the Eaglesong of the header is within the target encoded from `compact_target`.


### Type `HeaderView`

The JSON view of a Header.

This structure is serialized into a JSON object with field `hash` and all the fields in [`Header`](#type-header).

##### Examples


```
{
  "compact_target": "0x1e083126",
  "dao": "0xb5a3e047474401001bc476b9ee573000c0c387962a38000000febffacf030000",
  "epoch": "0x7080018000001",
  "hash": "0xa5f5c85987a15de25661e5a214f2c1449cd803f071acc7999820f25246471f40",
  "nonce": "0x0",
  "number": "0x400",
  "parent_hash": "0xae003585fa15309b30b31aed3dcf385e9472c3c3e93746a6c4540629a6a1ed2d",
  "proposals_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
  "timestamp": "0x5cd2b117",
  "transactions_root": "0xc47d5b78b3c4c4c853e2a32810818940d0ee403423bea9ec7b8e566d9595206c",
  "extra_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
  "version": "0x0"
}
```


#### Fields

`HeaderView` is a JSON object with the following fields.

*   `inner`: [`Header`](#type-header) - All the fields in `Header` are included in `HeaderView` in JSON.

*   `hash`: [`H256`](#type-h256) - The header hash. It is also called the block hash.


### Type `IndexerCell`

Live cell

#### Fields

`IndexerCell` is a JSON object with the following fields.

*   `output`: [`CellOutput`](#type-celloutput) - the fields of an output cell

*   `output_data`: [`JsonBytes`](#type-jsonbytes) `|` `null` - the cell data

*   `out_point`: [`OutPoint`](#type-outpoint) - reference to a cell via transaction hash and output index

*   `block_number`: [`BlockNumber`](#type-blocknumber) - the number of the transaction committed in the block

*   `tx_index`: [`Uint32`](#type-uint32) - the position index of the transaction committed in the block


### Type `IndexerCellsCapacity`

Cells capacity

#### Fields

`IndexerCellsCapacity` is a JSON object with the following fields.

*   `capacity`: [`Capacity`](#type-capacity) - total capacity

*   `block_hash`: [`H256`](#type-h256) - indexed tip block hash

*   `block_number`: [`BlockNumber`](#type-blocknumber) - indexed tip block number


### Type `IndexerOrder`

Order Desc | Asc

`IndexerOrder` is equivalent to `"desc" | "asc"`.

*   Descending order
*   Ascending order


### Type `IndexerRange`

A array represent (half-open) range bounded inclusively below and exclusively above [start, end).

##### Examples


|  JSON | range |
| --- |--- |
|  [“0x0”, “0x2”] | [0, 2) |
|  [“0x0”, “0x174876e801”] | [0, 100000000001) |




### Type `IndexerScriptType`

ScriptType `Lock` | `Type`

`IndexerScriptType` is equivalent to `"lock" | "type"`.

*   Lock
*   Type


### Type `IndexerSearchKey`

SearchKey represent indexer support params

#### Fields

`IndexerSearchKey` is a JSON object with the following fields.

*   `script`: [`Script`](#type-script) - Script, supports prefix search

*   `script_type`: [`IndexerScriptType`](#type-indexerscripttype) - Script Type

*   `filter`: [`IndexerSearchKeyFilter`](#type-indexersearchkeyfilter) `|` `null` - filter cells by following conditions, all conditions are optional

*   `with_data`: `boolean` `|` `null` - bool, optional default is `true`, if with_data is set to false, the field of returning cell.output_data is null in the result

*   `group_by_transaction`: `boolean` `|` `null` - bool, optional default is `false`, if group_by_transaction is set to true, the returning objects will be grouped by the tx hash


### Type `IndexerSearchKeyFilter`

IndexerSearchKeyFilter represent indexer params `filter`

#### Fields

`IndexerSearchKeyFilter` is a JSON object with the following fields.

*   `script`: [`Script`](#type-script) `|` `null` - if search script type is lock, filter cells by type script prefix, and vice versa

*   `script_len_range`: [`IndexerRange`](#type-indexerrange) `|` `null` - filter cells by script len range

*   `output_data_len_range`: [`IndexerRange`](#type-indexerrange) `|` `null` - filter cells by output data len range

*   `output_capacity_range`: [`IndexerRange`](#type-indexerrange) `|` `null` - filter cells by output capacity range

*   `block_range`: [`IndexerRange`](#type-indexerrange) `|` `null` - filter cells by block number range


### Type `IndexerTip`

Indexer tip information

#### Fields

`IndexerTip` is a JSON object with the following fields.

*   `block_hash`: [`H256`](#type-h256) - indexed tip block hash

*   `block_number`: [`BlockNumber`](#type-blocknumber) - indexed tip block number


### Type `IndexerTx`

Indexer Transaction Object

`IndexerTx` is equivalent to `"ungrouped" | "grouped"`.

*   ###### Ungrouped format represent as `IndexerTxWithCell`

    ####### Fields

    `IndexerCellType` is equivalent to `"input" | "output"`.

    `IndexerTxWithCell` is a JSON object with the following fields.

    *   `tx_hash`: [`H256`](#type-h256) - transaction hash

    *   `block_number`: [`BlockNumber`](#type-blocknumber) - the number of the transaction committed in the block

    *   `tx_index`: [`Uint32`](#type-uint32) - the position index of the transaction committed in the block

    *   `io_index`: [`Uint32`](#type-uint32) - the position index of the cell in the transaction inputs or outputs

    *   `io_type`: [`IndexerCellType`](#type-indexercelltype) - io type

*   ###### Grouped format represent as `IndexerTxWithCells`

    ####### Fields

    `IndexerCellType` is equivalent to `"input" | "output"`.

    `IndexerTxWithCells` is a JSON object with the following fields.

    *   `tx_hash`: [`H256`](#type-h256) - transaction hash

    *   `block_number`: [`BlockNumber`](#type-blocknumber) - the number of the transaction committed in the block

    *   `tx_index`: [`Uint32`](#type-uint32)- the position index of the transaction committed in the block

    *   `cells`: Array <(IndexerCellType, Uint32)>



### Type `JsonBytes`

Variable-length binary encoded as a 0x-prefixed hex string in JSON.

##### Example


|  JSON | Binary |
| --- |--- |
|  “0x” | Empty binary |
|  “0x00” | Single byte 0 |
|  “0x636b62” | 3 bytes, UTF-8 encoding of ckb |
|  “00” | Invalid, 0x is required |
|  “0x0” | Invalid, each byte requires 2 digits |




### Type `LocalNode`

The information of the node itself.

##### Examples


```
{
  "active": true,
  "addresses": [
    {
      "address": "/ip4/192.168.0.2/tcp/8112/p2p/QmTRHCdrRtgUzYLNCin69zEvPvLYdxUZLLfLYyHVY3DZAS",
      "score": "0xff"
    },
    {
      "address": "/ip4/0.0.0.0/tcp/8112/p2p/QmTRHCdrRtgUzYLNCin69zEvPvLYdxUZLLfLYyHVY3DZAS",
      "score": "0x1"
    }
  ],
  "connections": "0xb",
  "node_id": "QmTRHCdrRtgUzYLNCin69zEvPvLYdxUZLLfLYyHVY3DZAS",
  "protocols": [
    {
      "id": "0x0",
      "name": "/ckb/ping",
      "support_versions": [
        "0.0.1"
      ]
    },
    {
      "id": "0x1",
      "name": "/ckb/discovery",
      "support_versions": [
        "0.0.1"
      ]
    }
  ],
  "version": "0.34.0 (f37f598 2020-07-17)"
}
```


#### Fields

`LocalNode` is a JSON object with the following fields.

*   `version`: `string` - CKB node version.

    Example: “version”: “0.34.0 (f37f598 2020-07-17)”

*   `node_id`: `string` - The unique node ID derived from the p2p private key.

    The private key is generated randomly on the first boot.

*   `active`: `boolean` - Whether this node is active.

    An inactive node ignores incoming p2p messages and drops outgoing messages.

*   `addresses`: `Array<` [`NodeAddress`](#type-nodeaddress) `>` - P2P addresses of this node.

    A node can have multiple addresses.

*   `protocols`: `Array<` [`LocalNodeProtocol`](#type-localnodeprotocol) `>` - Supported protocols.

*   `connections`: [`Uint64`](#type-uint64) - Count of currently connected peers.


### Type `LocalNodeProtocol`

The information of a P2P protocol that is supported by the local node.

#### Fields

`LocalNodeProtocol` is a JSON object with the following fields.

*   `id`: [`Uint64`](#type-uint64) - Unique protocol ID.

*   `name`: `string` - Readable protocol name.

*   `support_versions`: `Array<` `string` `>` - Supported versions.

    See [Semantic Version](https://semver.org/) about how to specify a version.


### Type `MerkleProof`

Proof of CKB Merkle Tree.

CKB Merkle Tree is a [CBMT](https://github.com/nervosnetwork/rfcs/blob/master/rfcs/0006-merkle-tree/0006-merkle-tree.md) using CKB blake2b hash as the merge function.

#### Fields

`MerkleProof` is a JSON object with the following fields.

*   `indices`: `Array<` [`Uint32`](#type-uint32) `>` - Leaves indices in the CBMT that are proved present in the block.

    These are indices in the CBMT tree not the transaction indices in the block.

*   `lemmas`: `Array<` [`H256`](#type-h256) `>` - Hashes of all siblings along the paths to root.


### Type `MinerReward`

Block rewards for miners.

#### Fields

`MinerReward` is a JSON object with the following fields.

*   `primary`: [`Capacity`](#type-capacity) - The primary base block reward allocated to miners.

*   `secondary`: [`Capacity`](#type-capacity) - The secondary base block reward allocated to miners.

*   `committed`: [`Capacity`](#type-capacity) - The transaction fees that are rewarded to miners because the transaction is committed in the block.

    Miners get 60% of the transaction fee for each transaction committed in the block.

*   `proposal`: [`Capacity`](#type-capacity) - The transaction fees that are rewarded to miners because the transaction is proposed in the block or its uncles.

    Miners get 40% of the transaction fee for each transaction proposed in the block and committed later in its active commit window.


### Type `NodeAddress`

Node P2P address and score.

#### Fields

`NodeAddress` is a JSON object with the following fields.

*   `address`: `string` - P2P address.

    This is the same address used in the whitelist in ckb.toml.

    Example: “/ip4/192.168.0.2/tcp/8112/p2p/QmTRHCdrRtgUzYLNCin69zEvPvLYdxUZLLfLYyHVY3DZAS”

*   `score`: [`Uint64`](#type-uint64) - Address score.

    A higher score means a higher probability of a successful connection.


### Type `OutPoint`

Reference to a cell via transaction hash and output index.

##### Examples


```
{
  "index": "0x0",
  "tx_hash": "0x365698b50ca0da75dca2c87f9e7b563811d3b5813736b8cc62cc3b106faceb17"
}
```


#### Fields

`OutPoint` is a JSON object with the following fields.

*   `tx_hash`: [`H256`](#type-h256) - Transaction hash in which the cell is an output.

*   `index`: [`Uint32`](#type-uint32) - The output index of the cell in the transaction specified by `tx_hash`.


### Type `OutputsValidator`

Transaction output validators that prevent common mistakes.

`OutputsValidator` is equivalent to `"passthrough" | "well_known_scripts_only"`.

*   “passthrough”: the default validator, bypass output checking, thus allow any kind of transaction outputs.
*   “well_known_scripts_only”: restricts the lock script and type script usage, see more information on [https://github.com/nervosnetwork/ckb/wiki/Transaction-%C2%BB-Default-Outputs-Validator](https://github.com/nervosnetwork/ckb/wiki/Transaction-%C2%BB-Default-Outputs-Validator)


### Type `PeerSyncState`

The chain synchronization state between the local node and a remote node.

#### Fields

`PeerSyncState` is a JSON object with the following fields.

*   `best_known_header_hash`: [`Byte32`](#type-byte32) `|` `null` - Best known header hash of remote peer.

    This is the observed tip of the remote node’s canonical chain.

*   `best_known_header_number`: [`Uint64`](#type-uint64) `|` `null` - Best known header number of remote peer

    This is the block number of the block with the hash `best_known_header_hash`.

*   `last_common_header_hash`: [`Byte32`](#type-byte32) `|` `null` - Last common header hash of remote peer.

    This is the common ancestor of the local node canonical chain tip and the block `best_known_header_hash`.

*   `last_common_header_number`: [`Uint64`](#type-uint64) `|` `null` - Last common header number of remote peer.

    This is the block number of the block with the hash `last_common_header_hash`.

*   `unknown_header_list_size`: [`Uint64`](#type-uint64) - The total size of unknown header list.

    **Deprecated**: this is an internal state and will be removed in a future release.

*   `inflight_count`: [`Uint64`](#type-uint64) - The count of concurrency downloading blocks.

*   `can_fetch_count`: [`Uint64`](#type-uint64) - The count of blocks are available for concurrency download.


### Type `PoolTransactionEntry`

The transaction entry in the pool.

#### Fields

`PoolTransactionEntry` is a JSON object with the following fields.

*   `transaction`: [`TransactionView`](#type-transactionview) - The transaction.

*   `cycles`: [`Cycle`](#type-cycle) - Consumed cycles.

*   `size`: [`Uint64`](#type-uint64) - The transaction serialized size in block.

*   `fee`: [`Capacity`](#type-capacity) - The transaction fee.

*   `timestamp`: [`Uint64`](#type-uint64) - The unix timestamp when entering the Txpool, unit: Millisecond


### Type `PoolTransactionReject`

TX reject message

`PoolTransactionReject` is a JSON object with following fields.

*   `type`: `"LowFeeRate" | "ExceededMaximumAncestorsCount" | "Full" | "Duplicated" | "Malformed" | "DeclaredWrongCycles" | "Resolve" | "Verification" | "Expiry"` - Reject type.
*   `description`: `string` - Detailed description about why the transaction is rejected.

Different reject types:

*   `LowFeeRate`: Transaction fee lower than config
*   `ExceededMaximumAncestorsCount`: Transaction exceeded maximum ancestors count limit
*   `Full`: Transaction pool exceeded maximum size or cycles limit,
*   `Duplicated`: Transaction already exist in transaction_pool
*   `Malformed`: Malformed transaction
*   `DeclaredWrongCycles`: Declared wrong cycles
*   `Resolve`: Resolve failed
*   `Verification`: Verification failed
*   `Expiry`: Transaction expired


### Type `ProposalShortId`

The 10-byte fixed-length binary encoded as a 0x-prefixed hex string in JSON.

##### Example


```
0xa0ef4eb5f4ceeb08a4c8
```


### Type `ProposalWindow`

Two protocol parameters `closest` and `farthest` define the closest and farthest on-chain distance between a transaction’s proposal and commitment.

A non-cellbase transaction is committed at height h_c if all of the following conditions are met:

*   it is proposed at height h_p of the same chain, where w_close <= h_c − h_p <= w_far ;

*   it is in the commitment zone of the main chain block with height h_c ;


```
  ProposalWindow { closest: 2, farthest: 10 }
      propose
         \
          \
          13 14 [15 16 17 18 19 20 21 22 23]
                 \_______________________/
                              \
                            commit
```


#### Fields

`ProposalWindow` is a JSON object with the following fields.

*   `closest`: [`BlockNumber`](#type-blocknumber) - The closest distance between the proposal and the commitment.

*   `farthest`: [`BlockNumber`](#type-blocknumber) - The farthest distance between the proposal and the commitment.


### Type `RationalU256`

The ratio which numerator and denominator are both 256-bit unsigned integers.

#### Example

```
{
    "denom": "0x28",
    "numer": "0x1"
}
```


### Type `RawTxPool`

All transactions in tx-pool.

`RawTxPool` is equivalent to [`TxPoolIds`](#type-txpoolids) `|` [`TxPoolEntries`](#type-txpoolentries).

### Type `RemoteNode`

Information of a remote node.

A remote node connects to the local node via the P2P network. It is often called a peer.

##### Examples


```
{
  "addresses": [
    {
      "address": "/ip6/::ffff:18.185.102.19/tcp/8115/p2p/QmXwUgF48ULy6hkgfqrEwEfuHW7WyWyWauueRDAYQHNDfN",
      "score": "0x64"
    },
    {
      "address": "/ip4/18.185.102.19/tcp/8115/p2p/QmXwUgF48ULy6hkgfqrEwEfuHW7WyWyWauueRDAYQHNDfN",
      "score": "0x64"
    }
  ],
  "connected_duration": "0x2f",
  "is_outbound": true,
  "last_ping_duration": "0x1a",
  "node_id": "QmXwUgF48ULy6hkgfqrEwEfuHW7WyWyWauueRDAYQHNDfN",
  "protocols": [
    {
      "id": "0x4",
      "version": "0.0.1"
    },
    {
      "id": "0x2",
      "version": "0.0.1"
    },
    {
      "id": "0x1",
      "version": "0.0.1"
    },
    {
      "id": "0x64",
      "version": "1"
    },
    {
      "id": "0x6e",
      "version": "1"
    },
    {
      "id": "0x66",
      "version": "1"
    },
    {
      "id": "0x65",
      "version": "1"
    },
    {
      "id": "0x0",
      "version": "0.0.1"
    }
  ],
  "sync_state": {
    "best_known_header_hash": null,
    "best_known_header_number": null,
    "can_fetch_count": "0x80",
    "inflight_count": "0xa",
    "last_common_header_hash": null,
    "last_common_header_number": null,
    "unknown_header_list_size": "0x20"
  },
  "version": "0.34.0 (f37f598 2020-07-17)"
}
```


#### Fields

`RemoteNode` is a JSON object with the following fields.

*   `version`: `string` - The remote node version.

*   `node_id`: `string` - The remote node ID which is derived from its P2P private key.

*   `addresses`: `Array<` [`NodeAddress`](#type-nodeaddress) `>` - The remote node addresses.

*   `is_outbound`: `boolean` - Whether this is an outbound remote node.

    If the connection is established by the local node, `is_outbound` is true.

*   `connected_duration`: [`Uint64`](#type-uint64) - Elapsed time in milliseconds since the remote node is connected.

*   `last_ping_duration`: [`Uint64`](#type-uint64) `|` `null` - Elapsed time in milliseconds since receiving the ping response from this remote node.

    Null means no ping responses have been received yet.

*   `sync_state`: [`PeerSyncState`](#type-peersyncstate) `|` `null` - Chain synchronization state.

    Null means chain sync has not started with this remote node yet.

*   `protocols`: `Array<` [`RemoteNodeProtocol`](#type-remotenodeprotocol) `>` - Active protocols.

    CKB uses Tentacle multiplexed network framework. Multiple protocols are running simultaneously in the connection.


### Type `RemoteNodeProtocol`

The information about an active running protocol.

#### Fields

`RemoteNodeProtocol` is a JSON object with the following fields.

*   `id`: [`Uint64`](#type-uint64) - Unique protocol ID.

*   `version`: `string` - Active protocol version.


### Type `ResponseFormat`

This is a wrapper for JSON serialization to select the format between Json and Hex.

##### Examples

`ResponseFormat<BlockView>` returns the block in its Json format or molecule serialized Hex format.

#### Fields

`ResponseFormat` is a JSON object with the following fields.

*   `inner`: [`Either`](#type-either) - The inner value.


### Type `Script`

Describes the lock script and type script for a cell.

##### Examples


```
{
  "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
  "hash_type": "data",
  "args": "0x"
}
```


#### Fields

`Script` is a JSON object with the following fields.

*   `code_hash`: [`H256`](#type-h256) - The hash used to match the script code.

*   `hash_type`: [`ScriptHashType`](#type-scripthashtype) - Specifies how to use the `code_hash` to match the script code.

*   `args`: [`JsonBytes`](#type-jsonbytes) - Arguments for script.


### Type `ScriptHashType`

Specifies how the script `code_hash` is used to match the script code and how to run the code.

Allowed kinds: “data”, “type” and “data1”.

Refer to the section [Code Locating](https://github.com/nervosnetwork/rfcs/blob/master/rfcs/0022-transaction-structure/0022-transaction-structure.md#code-locating) and [Upgradable Script](https://github.com/nervosnetwork/rfcs/blob/master/rfcs/0022-transaction-structure/0022-transaction-structure.md#upgradable-script) in the RFC *CKB Transaction Structure*.

`ScriptHashType` is equivalent to `"data" | "type" | "data1"`.

*   Type “data” matches script code via cell data hash, and run the script code in v0 CKB VM.
*   Type “type” matches script code via cell type script hash.
*   Type “data1” matches script code via cell data hash, and run the script code in v1 CKB VM.


### Type `SerializedBlock`

This is a 0x-prefix hex string. It is the block serialized by molecule using the schema `table Block`.

### Type `SerializedHeader`

This is a 0x-prefix hex string. It is the block header serialized by molecule using the schema `table Header`.

### Type `Status`

Status for transaction

`Status` is equivalent to `"pending" | "proposed" | "committed" | "unknown" | "rejected"`.

*   Status “pending”. The transaction is in the pool, and not proposed yet.
*   Status “proposed”. The transaction is in the pool and has been proposed.
*   Status “committed”. The transaction has been committed to the canonical chain.
*   Status “unknown”. The node has not seen the transaction, or it should be rejected but was cleared due to storage limitations.
*   Status “rejected”. The transaction has been recently removed from the pool. Due to storage limitations, the node can only hold the most recently removed transactions.


### Type `SyncState`

The overall chain synchronization state of this local node.

#### Fields

`SyncState` is a JSON object with the following fields.

*   `ibd`: `boolean` - Whether the local node is in IBD, Initial Block Download.

    When a node starts and its chain tip timestamp is far behind the wall clock, it will enter the IBD until it catches up the synchronization.

    During IBD, the local node only synchronizes the chain with one selected remote node and stops responding to most P2P requests.

*   `best_known_block_number`: [`BlockNumber`](#type-blocknumber) - This is the best known block number observed by the local node from the P2P network.

    The best here means that the block leads a chain which has the best known accumulated difficulty.

    This can be used to estimate the synchronization progress. If this RPC returns B, and the RPC `get_tip_block_number` returns T, the node has already synchronized T/B blocks.

*   `best_known_block_timestamp`: [`Timestamp`](#type-timestamp) - This is timestamp of the same block described in `best_known_block_number`.

*   `orphan_blocks_count`: [`Uint64`](#type-uint64) - Count of orphan blocks the local node has downloaded.

    The local node downloads multiple blocks simultaneously but blocks must be connected consecutively. If a descendant is downloaded before its ancestors, it becomes an orphan block.

    If this number is too high, it indicates that block download has stuck at some block.

*   `inflight_blocks_count`: [`Uint64`](#type-uint64) - Count of downloading blocks.

*   `fast_time`: [`Uint64`](#type-uint64) - The download scheduler’s time analysis data, the fast is the 1/3 of the cut-off point, unit ms

*   `normal_time`: [`Uint64`](#type-uint64) - The download scheduler’s time analysis data, the normal is the 4/5 of the cut-off point, unit ms

*   `low_time`: [`Uint64`](#type-uint64) - The download scheduler’s time analysis data, the low is the 9/10 of the cut-off point, unit ms


### Type `Timestamp`

The Unix timestamp in milliseconds (1 second is 1000 milliseconds).

For example, 1588233578000 is Thu, 30 Apr 2020 07:59:38 +0000

This is a 64-bit unsigned integer type encoded as the 0x-prefixed hex string in JSON. See examples of [Uint64](#type-uint64).

### Type `Transaction`

The transaction.

Refer to RFC [CKB Transaction Structure](https://github.com/nervosnetwork/rfcs/blob/master/rfcs/0022-transaction-structure/0022-transaction-structure.md).

#### Fields

`Transaction` is a JSON object with the following fields.

*   `version`: [`Version`](#type-version) - Reserved for future usage. It must equal 0 in current version.

*   `cell_deps`: `Array<` [`CellDep`](#type-celldep) `>` - An array of cell deps.

    CKB locates lock script and type script code via cell deps. The script also can uses syscalls to read the cells here.

    Unlike inputs, the live cells can be used as cell deps in multiple transactions.

*   `header_deps`: `Array<` [`H256`](#type-h256) `>` - An array of header deps.

    The block must already be in the canonical chain.

    Lock script and type script can read the header information of blocks listed here.

*   `inputs`: `Array<` [`CellInput`](#type-cellinput) `>` - An array of input cells.

    In the canonical chain, any cell can only appear as an input once.

*   `outputs`: `Array<` [`CellOutput`](#type-celloutput) `>` - An array of output cells.

*   `outputs_data`: `Array<` [`JsonBytes`](#type-jsonbytes) `>` - Output cells data.

    This is a parallel array of outputs. The cell capacity, lock, and type of the output i is `outputs[i]` and its data is `outputs_data[i]`.

*   `witnesses`: `Array<` [`JsonBytes`](#type-jsonbytes) `>` - An array of variable-length binaries.

    Lock script and type script can read data here to verify the transaction.

    For example, the bundled secp256k1 lock script requires storing the signature in `witnesses`.


### Type `TransactionProof`

Merkle proof for transactions in a block.

#### Fields

`TransactionProof` is a JSON object with the following fields.

*   `block_hash`: [`H256`](#type-h256) - Block hash

*   `witnesses_root`: [`H256`](#type-h256) - Merkle root of all transactions’ witness hash

*   `proof`: [`MerkleProof`](#type-merkleproof) - Merkle proof of all transactions’ hash


### Type `TransactionTemplate`

Transaction template which is ready to be committed in the new block.

#### Fields

`TransactionTemplate` is a JSON object with the following fields.

*   `hash`: [`H256`](#type-h256) - Transaction hash.

*   `required`: `boolean` - Whether miner must include this transaction in the new block.

*   `cycles`: [`Cycle`](#type-cycle) `|` `null` - The hint of how many cycles this transaction consumes.

    Miners can utilize this field to ensure that the total cycles do not exceed the limit while selecting transactions.

*   `depends`: `Array<` [`Uint64`](#type-uint64) `>` `|` `null` - Transaction dependencies.

    This is a hint to help miners selecting transactions.

    This transaction can only be committed if its dependencies are also committed in the new block.

    This field is a list of indices into the array `transactions` in the block template.

    For example, `depends = [1, 2]` means this transaction depends on `block_template.transactions[1]` and `block_template.transactions[2]`.

*   `data`: [`Transaction`](#type-transaction) - The transaction.

    Miners must keep it unchanged when including it in the new block.


### Type `TransactionView`

The JSON view of a Transaction.

This structure is serialized into a JSON object with field `hash` and all the fields in [`Transaction`](#type-transaction).

##### Examples


```
{
  "cell_deps": [
    {
      "dep_type": "code",
      "out_point": {
        "index": "0x0",
        "tx_hash": "0xa4037a893eb48e18ed4ef61034ce26eba9c585f15c9cee102ae58505565eccc3"
      }
    }
  ],
  "hash": "0xa0ef4eb5f4ceeb08a4c8524d84c5da95dce2f608e0ca2ec8091191b0f330c6e3",
  "header_deps": [
    "0x7978ec7ce5b507cfb52e149e36b1a23f6062ed150503c85bbf825da3599095ed"
  ],
  "inputs": [
    {
      "previous_output": {
        "index": "0x0",
        "tx_hash": "0x365698b50ca0da75dca2c87f9e7b563811d3b5813736b8cc62cc3b106faceb17"
      },
      "since": "0x0"
    }
  ],
  "outputs": [
    {
      "capacity": "0x2540be400",
      "lock": {
        "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
        "hash_type": "data",
        "args": "0x"
      },
      "type": null
    }
  ],
  "outputs_data": [
    "0x"
  ],
  "version": "0x0",
  "witnesses": []
}
```


#### Fields

`TransactionView` is a JSON object with the following fields.

*   `inner`: [`Transaction`](#type-transaction) - All the fields in `Transaction` are included in `TransactionView` in JSON.

*   `hash`: [`H256`](#type-h256) - The transaction hash.


### Type `TransactionWithStatusResponse`

The JSON view of a transaction as well as its status.

#### Fields

`TransactionWithStatusResponse` is a JSON object with the following fields.

*   `transaction`: [`ResponseFormat`](#type-responseformat) `|` `null` - The transaction.

*   `cycles`: [`Cycle`](#type-cycle) `|` `null` - The transaction consumed cycles.

*   `tx_status`: [`TxStatus`](#type-txstatus) - The Transaction status.


### Type `TxPoolEntries`

Tx-pool entries object

#### Fields

`TxPoolEntries` is a JSON object with the following fields.

*   `pending`: `{ [ key:` [`H256`](#type-h256) `]: ` [`TxPoolEntry`](#type-txpoolentry) `}` - Pending tx verbose info

*   `proposed`: `{ [ key:` [`H256`](#type-h256) `]: ` [`TxPoolEntry`](#type-txpoolentry) `}` - Proposed tx verbose info


### Type `TxPoolEntry`

Transaction entry info

#### Fields

`TxPoolEntry` is a JSON object with the following fields.

*   `cycles`: [`Uint64`](#type-uint64) - Consumed cycles.

*   `size`: [`Uint64`](#type-uint64) - The transaction serialized size in block.

*   `fee`: [`Capacity`](#type-capacity) - The transaction fee.

*   `ancestors_size`: [`Uint64`](#type-uint64) - Size of in-tx-pool ancestor transactions

*   `ancestors_cycles`: [`Uint64`](#type-uint64) - Cycles of in-tx-pool ancestor transactions

*   `ancestors_count`: [`Uint64`](#type-uint64) - Number of in-tx-pool ancestor transactions

*   `timestamp`: [`Uint64`](#type-uint64) - The unix timestamp when entering the Txpool, unit: Millisecond


### Type `TxPoolIds`

Array of transaction ids

#### Fields

`TxPoolIds` is a JSON object with the following fields.

*   `pending`: `Array<` [`H256`](#type-h256) `>` - Pending transaction ids

*   `proposed`: `Array<` [`H256`](#type-h256) `>` - Proposed transaction ids


### Type `TxPoolInfo`

Transaction pool information.

#### Fields

`TxPoolInfo` is a JSON object with the following fields.

*   `tip_hash`: [`H256`](#type-h256) - The associated chain tip block hash.

    The transaction pool is stateful. It manages the transactions which are valid to be committed after this block.

*   `tip_number`: [`BlockNumber`](#type-blocknumber) - The block number of the block `tip_hash`.

*   `pending`: [`Uint64`](#type-uint64) - Count of transactions in the pending state.

    The pending transactions must be proposed in a new block first.

*   `proposed`: [`Uint64`](#type-uint64) - Count of transactions in the proposed state.

    The proposed transactions are ready to be committed in the new block after the block `tip_hash`.

*   `orphan`: [`Uint64`](#type-uint64) - Count of orphan transactions.

    An orphan transaction has an input cell from the transaction which is neither in the chain nor in the transaction pool.

*   `total_tx_size`: [`Uint64`](#type-uint64) - Total count of transactions in the pool of all the different kinds of states (excluding orphan transactions).

*   `total_tx_cycles`: [`Uint64`](#type-uint64) - Total consumed VM cycles of all the transactions in the pool (excluding orphan transactions).

*   `min_fee_rate`: [`Uint64`](#type-uint64) - Fee rate threshold. The pool rejects transactions which fee rate is below this threshold.

    The unit is Shannons per 1000 bytes transaction serialization size in the block.

*   `last_txs_updated_at`: [`Timestamp`](#type-timestamp) - Last updated time. This is the Unix timestamp in milliseconds.


### Type `TxStatus`

Transaction status and the block hash if it is committed.

#### Fields

`TxStatus` is a JSON object with the following fields.

*   `status`: [`Status`](#type-status) - The transaction status, allowed values: “pending”, “proposed” “committed” “unknown” and “rejected”.

*   `block_hash`: [`H256`](#type-h256) `|` `null` - The block hash of the block which has committed this transaction in the canonical chain.

*   `reason`: `string` `|` `null` - The reason why the transaction is rejected


### Type `U256`

The 256-bit unsigned integer type encoded as the 0x-prefixed hex string in JSON.

### Type `Uint128`

The  128-bit unsigned integer type encoded as the 0x-prefixed hex string in JSON.

##### Examples


|  JSON | Decimal Value |
| --- |--- |
|  “0x0” | 0 |
|  “0x10” | 16 |
|  “10” | Invalid, 0x is required |
|  “0x01” | Invalid, redundant leading 0 |


### Type `Uint32`

The  32-bit unsigned integer type encoded as the 0x-prefixed hex string in JSON.

##### Examples


|  JSON | Decimal Value |
| --- |--- |
|  “0x0” | 0 |
|  “0x10” | 16 |
|  “10” | Invalid, 0x is required |
|  “0x01” | Invalid, redundant leading 0 |


### Type `Uint64`

The  64-bit unsigned integer type encoded as the 0x-prefixed hex string in JSON.

##### Examples


|  JSON | Decimal Value |
| --- |--- |
|  “0x0” | 0 |
|  “0x10” | 16 |
|  “10” | Invalid, 0x is required |
|  “0x01” | Invalid, redundant leading 0 |


### Type `UncleBlock`

The uncle block used as a parameter in the RPC.

The chain stores only the uncle block header and proposal IDs. The header ensures the block is covered by PoW and can pass the consensus rules on uncle blocks. Proposal IDs are there because a block can commit transactions proposed in an uncle.

A block B1 is considered to be the uncle of another block B2 if all the following conditions are met:

*   They are in the same epoch, sharing the same difficulty;

*   B2 block number is larger than B1;

*   B1’s parent is either B2’s ancestor or an uncle embedded in B2 or any of B2’s ancestors.

*   B2 is the first block in its chain to refer to B1.

#### Fields

`UncleBlock` is a JSON object with the following fields.

*   `header`: [`Header`](#type-header) - The uncle block header.

*   `proposals`: `Array<` [`ProposalShortId`](#type-proposalshortid) `>` - Proposal IDs in the uncle block body.


### Type `UncleBlockView`

The uncle block.

The chain stores only the uncle block header and proposal IDs. The header ensures the block is covered by PoW and can pass the consensus rules on uncle blocks. Proposal IDs are there because a block can commit transactions proposed in an uncle.

A block B1 is considered to be the uncle of another block B2 if all the following conditions are met:

*   They are in the same epoch, sharing the same difficulty;

*   B2 block number is larger than B1;

*   B1’s parent is either B2’s ancestor or an uncle embedded in B2 or any of B2’s ancestors.

*   B2 is the first block in its chain to refer to B1.

#### Fields

`UncleBlockView` is a JSON object with the following fields.

*   `header`: [`HeaderView`](#type-headerview) - The uncle block header.

*   `proposals`: `Array<` [`ProposalShortId`](#type-proposalshortid) `>` - Proposal IDs in the uncle block body.


### Type `UncleTemplate`

The uncle block template of the new block for miners.

#### Fields

`UncleTemplate` is a JSON object with the following fields.

*   `hash`: [`H256`](#type-h256) - The uncle block hash.

*   `required`: `boolean` - Whether miners must include this uncle in the submit block.

*   `proposals`: `Array<` [`ProposalShortId`](#type-proposalshortid) `>` - The proposals of the uncle block.

    Miners must keep this unchanged when including this uncle in the new block.

*   `header`: [`Header`](#type-header) - The header of the uncle block.

    Miners must keep this unchanged when including this uncle in the new block.


### Type `Version`

The simple increasing integer version.

This is a 32-bit unsigned integer type encoded as the 0x-prefixed hex string in JSON. See examples of [Uint32](#type-uint32).
