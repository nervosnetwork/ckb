# CKB JSON-RPC Protocols

<!--**NOTE:** This file is auto-generated from code comments.-->

The RPC interface shares the version of the node version, which is returned in `local_node_info`. The interface is fully compatible between patch versions, for example, a client for 0.25.0 should work with 0.25.x for any x.

Allowing arbitrary machines to access the JSON-RPC port (using the `rpc.listen_address` configuration option) is **dangerous and strongly discouraged**. Please strictly limit the access to only trusted machines.

CKB JSON-RPC only supports HTTP now. If you need SSL, please setup a proxy via Nginx or other HTTP servers.

Subscriptions require a full duplex connection. CKB offers such connections in the form of TCP (enable with `rpc.tcp_listen_address` configuration option) and WebSockets (enable with `rpc.ws_listen_address`).

# JSONRPC Deprecation Process

A CKB RPC method is deprecated in three steps.

First the method is marked as deprecated in the CKB release notes and RPC document. However, the RPC method is still available. The RPC document will have the suggestion of the alternative solutions.

The CKB dev team will disable any deprecated RPC methods starting from the next minor version release. Users can enable the deprecated methods via the config file option rpc.enable_deprecated_rpc.

Once a deprecated method is disabled, the CKB dev team will remove it in a future minor version release.

For example, a method is marked as deprecated in 0.35.0, it can be disabled in 0.36.0 and removed in 0.37.0. The minor versions are released monthly, so there's at least two month buffer for a deprecated RPC method.


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

##### Errors

*   [`AlertFailedToVerifySignatures (-1000)`](#error-alertfailedtoverifysignatures) - Some signatures in the request are invalid.

*   [`P2PFailedToBroadcast (-101)`](#error-p2pfailedtobroadcast) - Alert is saved locally but has failed to broadcast to the P2P network.

*   `InvalidParams (-32602)` - The time specified in `alert.notice_until` must be in the future.

##### Examples

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

#### Canonical Chain

A canonical chain is the one with the most accumulated work. The accumulated work is the sum of difficulties of all the blocks in the chain.

#### Chain Reorganization

Chain Reorganization happens when CKB found a chain which has accumulated more work than the canonical chain. The reorganization reverts the blocks in the current canonical chain if needed, and switch the canonical chain to that better chain.

#### Live Cell

A cell is live if

*   it is found as an output in any transaction in the [canonical chain](#canonical-chain), and

*   it is not found as an input in any transaction in the canonical chain.

#### Method `get_block`
* `get_block(block_hash, verbosity)`
    * `block_hash`: [`H256`](#type-h256)
    * `verbosity`: [`Uint32`](#type-uint32) `|` `null`
* result: [`BlockView`](#type-blockview) `|` [`SerializedBlock`](#type-serializedblock) `|` `null`

Returns the information about a block by hash.

##### Params

*   `block_hash` - the block hash.

*   `verbosity` - result format which allows 0 and 2. (**Optional**, the default is 2.)

##### Returns

The RPC returns a block or null. When the RPC returns a block, the block hash must equal to the parameter `block_hash`.

If the block is in the [canonical chain](#canonical-chain), the RPC must return the block information. Otherwise the behavior is undefined. The RPC may return blocks found in local storage, or simply returns null for all blocks that are not in the canonical chain. And because of [chain reorganization](#chain-reorganization), for the same `block_hash`, the RPC may sometimes return null and sometimes return the block.

When `verbosity` is 2, it returns a JSON object as the `result`. See `BlockView` for the schema.

When `verbosity` is 0, it returns a 0x-prefixed hex string as the `result`. The string encodes the block serialized by molecule using schema `table Block`.

##### Examples

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
      "hash": "0xa5f5c85987a15de25661e5a214f2c1449cd803f071acc7999820f25246471f40",
      "nonce": "0x0",
      "number": "0x400",
      "parent_hash": "0xae003585fa15309b30b31aed3dcf385e9472c3c3e93746a6c4540629a6a1ed2d",
      "proposals_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
      "timestamp": "0x5cd2b117",
      "transactions_root": "0xc47d5b78b3c4c4c853e2a32810818940d0ee403423bea9ec7b8e566d9595206c",
      "uncles_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
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
              "args": "0x",
              "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
              "hash_type": "data"
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

#### Method `get_block_by_number`
* `get_block_by_number(block_number, verbosity)`
    * `block_number`: [`BlockNumber`](#type-blocknumber)
    * `verbosity`: [`Uint32`](#type-uint32) `|` `null`
* result: [`BlockView`](#type-blockview) `|` [`SerializedBlock`](#type-serializedblock) `|` `null`

Returns the block in the [canonical chain](#canonical-chain) with the specific block number.

##### Params

*   `block_number` - the block number.

*   `verbosity` - result format which allows 0 and 2. (**Optional**, the default is 2.)

##### Returns

The RPC returns the block when `block_number` is less than or equal to the tip block number returned by [`get_tip_block_number`](#method-get_tip_block_number) and returns null otherwise.

Because of [chain reorganization](#chain-reorganization), the PRC may return null or even different blocks in different invocations with the same `block_number`.

When `verbosity` is 2, it returns a JSON object as the `result`. See `BlockView` for the schema.

When `verbosity` is 0, it returns a 0x-prefixed hex string as the `result`. The string encodes the block serialized by molecule using schema `table Block`.

##### Errors

*   [`ChainIndexIsInconsistent (-201)`](#error-chainindexisinconsistent) - The index is inconsistent. It says a block hash is in the main chain, but cannot read it from database.

*   [`DatabaseIsCorrupt (-202)`](#error-databaseiscorrupt) - The data read from database is dirty. Please report it as a bug.

##### Examples

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
      "hash": "0xa5f5c85987a15de25661e5a214f2c1449cd803f071acc7999820f25246471f40",
      "nonce": "0x0",
      "number": "0x400",
      "parent_hash": "0xae003585fa15309b30b31aed3dcf385e9472c3c3e93746a6c4540629a6a1ed2d",
      "proposals_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
      "timestamp": "0x5cd2b117",
      "transactions_root": "0xc47d5b78b3c4c4c853e2a32810818940d0ee403423bea9ec7b8e566d9595206c",
      "uncles_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
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
              "args": "0x",
              "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
              "hash_type": "data"
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

#### Method `get_header`
* `get_header(block_hash, verbosity)`
    * `block_hash`: [`H256`](#type-h256)
    * `verbosity`: [`Uint32`](#type-uint32) `|` `null`
* result: [`HeaderView`](#type-headerview) `|` [`SerializedHeader`](#type-serializedheader) `|` `null`

Returns the information about a block header by hash.

##### Params

*   `block_hash` - the block hash.

*   `verbosity` - result format which allows 0 and 1. (**Optional**, the default is 1.)

##### Returns

The RPC returns a header or null. When the RPC returns a header, the block hash must equal to the parameter `block_hash`.

If the block is in the [canonical chain](#canonical-chain), the RPC must return the header information. Otherwise the behavior is undefined. The RPC may return blocks found in local storage, or simply returns null for all blocks that are not in the canonical chain. And because of [chain reorganization](#chain-reorganization), for the same `block_hash`, the RPC may sometimes return null and sometimes return the block header.

When `verbosity` is 1, it returns a JSON object as the `result`. See `HeaderView` for the schema.

When `verbosity` is 0, it returns a 0x-prefixed hex string as the `result`. The string encodes the block header serialized by molecule using schema `table Header`.

##### Examples

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
    "hash": "0xa5f5c85987a15de25661e5a214f2c1449cd803f071acc7999820f25246471f40",
    "nonce": "0x0",
    "number": "0x400",
    "parent_hash": "0xae003585fa15309b30b31aed3dcf385e9472c3c3e93746a6c4540629a6a1ed2d",
    "proposals_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
    "timestamp": "0x5cd2b117",
    "transactions_root": "0xc47d5b78b3c4c4c853e2a32810818940d0ee403423bea9ec7b8e566d9595206c",
    "uncles_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
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

##### Params

*   `block_number` - Number of a block

*   `verbosity` - result format which allows 0 and 1. (**Optional**, the default is 1.)

##### Returns

The RPC returns the block header when `block_number` is less than or equal to the tip block number returned by [`get_tip_block_number`](#method-get_tip_block_number) and returns null otherwise.

Because of [chain reorganization](#chain-reorganization), the PRC may return null or even different block headers in different invocations with the same `block_number`.

When `verbosity` is 1, it returns a JSON object as the `result`. See `HeaderView` for the schema.

When `verbosity` is 0, it returns a 0x-prefixed hex string as the `result`. The string encodes the block header serialized by molecule using schema `table Header`.

##### Errors

*   [`ChainIndexIsInconsistent (-201)`](#error-chainindexisinconsistent) - The index is inconsistent. It says a block hash is in the main chain, but cannot read it from database.

##### Examples

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
    "hash": "0xa5f5c85987a15de25661e5a214f2c1449cd803f071acc7999820f25246471f40",
    "nonce": "0x0",
    "number": "0x400",
    "parent_hash": "0xae003585fa15309b30b31aed3dcf385e9472c3c3e93746a6c4540629a6a1ed2d",
    "proposals_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
    "timestamp": "0x5cd2b117",
    "transactions_root": "0xc47d5b78b3c4c4c853e2a32810818940d0ee403423bea9ec7b8e566d9595206c",
    "uncles_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
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

#### Method `get_transaction`
* `get_transaction(tx_hash)`
    * `tx_hash`: [`H256`](#type-h256)
* result: [`TransactionWithStatus`](#type-transactionwithstatus) `|` `null`

Returns the information about a transaction requested by transaction hash.

##### Returns

This RPC returns `null` if the transaction is not committed in the[canonical chain](#canonical-chain) nor in the transaction memory pool.

If the transaction is in the chain, the block hash is also returned.

##### Params

*   `tx_hash` - Hash of a transaction

##### Examples

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
            "args": "0x",
            "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
            "hash_type": "data"
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
    "tx_status": {
      "block_hash": null,
      "status": "pending"
    }
  }
}
```

#### Method `get_block_hash`
* `get_block_hash(block_number)`
    * `block_number`: [`BlockNumber`](#type-blocknumber)
* result: [`H256`](#type-h256) `|` `null`

Returns the hash of a block in the [canonical chain](#canonical-chain) with the specified`block_number`.

##### Params

*   `block_number` - Block number

##### Returns

The RPC returns the block hash when `block_number` is less than or equal to the tip block number returned by [`get_tip_block_number`](#method-get_tip_block_number) and returns null otherwise.

Because of [chain reorganization](#chain-reorganization), the PRC may return null or even different block hashes in different invocations with the same `block_number`.

##### Examples

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

##### Params

*   `verbosity` - result format which allows 0 and 1. (**Optional**, the default is 1.)

##### Returns

When `verbosity` is 1, the RPC returns a JSON object as the `result`. See HeaderView for the schema.

When `verbosity` is 0, it returns a 0x-prefixed hex string as the `result`. The string encodes the header serialized by molecule using schema `table Header`.

##### Examples

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
    "hash": "0xa5f5c85987a15de25661e5a214f2c1449cd803f071acc7999820f25246471f40",
    "nonce": "0x0",
    "number": "0x400",
    "parent_hash": "0xae003585fa15309b30b31aed3dcf385e9472c3c3e93746a6c4540629a6a1ed2d",
    "proposals_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
    "timestamp": "0x5cd2b117",
    "transactions_root": "0xc47d5b78b3c4c4c853e2a32810818940d0ee403423bea9ec7b8e566d9595206c",
    "uncles_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
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

#### Method `get_cells_by_lock_hash`
* `get_cells_by_lock_hash(lock_hash, from, to)`
    * `lock_hash`: [`H256`](#type-h256)
    * `from`: [`BlockNumber`](#type-blocknumber)
    * `to`: [`BlockNumber`](#type-blocknumber)
* result: `Array<`[`CellOutputWithOutPoint`](#type-celloutputwithoutpoint)`>`

👎 Deprecated since 0.36.0:
(Disabled since 0.36.0) This method is deprecated for reasons of flexibility. Please use [ckb-indexer](https://github.com/nervosnetwork/ckb-indexer) as an alternate solution


#### Method `get_live_cell`
* `get_live_cell(out_point, with_data)`
    * `out_point`: [`OutPoint`](#type-outpoint)
    * `with_data`: `boolean`
* result: [`CellWithStatus`](#type-cellwithstatus)

Returns the status about a cell. The RPC returns extra information if it is a [live cell] (#live-cell).

##### Returns

This RPC tells whether a cell is live or not.

If the cell is live, the RPC will return details about the cell. Otherwise the field `cell` is null in the result.

If the cell is live and `with_data` is set to `false`, the field `cell.data` is null in the result.

##### Params

*   `out_point` - Reference to the cell by transaction hash and output index.

*   `with_data` - Whether the RPC should return cell data. Cell data can be huge, if client does not need the data, it should set this to `false` to save bandwidth.

##### Examples

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
          "args": "0x",
          "code_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
          "hash_type": "data"
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

##### Examples

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

##### Examples

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

##### Params

*   `epoch_number` - Epoch number

##### Returns

The RPC returns the epoch when `epoch_number` is less than or equal to the current epoch number returned by [`get_current_epoch`](#method-get_current_epoch) and returns null otherwise.

Because of [chain reorganization](#chain-reorganization), for the same `epoch_number`, this RPC may return null or different epochs in different invocations.

##### Examples

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

#### Method `get_cellbase_output_capacity_details`
* `get_cellbase_output_capacity_details(block_hash)`
    * `block_hash`: [`H256`](#type-h256)
* result: [`BlockReward`](#type-blockreward) `|` `null`

👎 Deprecated since 0.36.0:
Please use the RPC method [`get_block_economic_state`](#method-get_block_economic_state) instead


#### Method `get_block_economic_state`
* `get_block_economic_state(block_hash)`
    * `block_hash`: [`H256`](#type-h256)
* result: [`BlockEconomicState`](#type-blockeconomicstate) `|` `null`

Returns increased issuance, miner reward and total transaction fee of a block.

This RPC returns null if the block is not in the [canonical chain](#canonical-chain).

CKB delays CKB creation for miners. The output cells in the cellbase of block N are for the miner creating block `N - 1 - ProposalWindow.farthest`.

In mainnet, `ProposalWindow.farthest` is 10, so the outputs in block 100 are rewards for miner creating block 89.

Because of the delay, this RPC returns null if the block rewards are not finalized yet. For example, the economic state for block 89 is only available when the number returned by[`get_tip_block_number`](#method-get_tip_block_number) is greater than or equal to 100.

##### Params

*   `block_hash` - Specifies the block hash which rewards should be analyzed.

##### Returns

If the block with the hash `block_hash` is in the [canonical chain](#canonical-chain) and its rewards have been finalized, return the block rewards analysis for this block.

##### Examples

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
    * `tx_hashes`: `Array<`[`H256`](#type-h256)`>`
    * `block_hash`: [`H256`](#type-h256) `|` `null`
* result: [`TransactionProof`](#type-transactionproof)

Returns a merkle proof that transaction was included in a block.

##### Params

*   `tx_hashes` - Transaction hashes, all transactions must be in the same block

*   `block_hash` - An optional parameter, if specified, looks for transactions in the block with this hash

##### Examples

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
* result: `Array<`[`H256`](#type-h256)`>`

Verifies that a proof points to transactions in a block, returning the transaction hashes it commits to.

##### Parameters

*   `transaction_proof` - proof generated by [`get_transaction_proof`](#method-get_transaction_proof).

##### Examples

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

### Module Debug

RPC Module Debug for internal RPC methods.

**This module is for CKB developers and will not guarantee compatibility.** The methods here will be changed or removed without advanced notification.

#### Method `jemalloc_profiling_dump`
* `jemalloc_profiling_dump()`
* result: `string`

Dumps jemalloc memory profiling information into a file.

The file is stored in the server running the CKB node.

The RPC returns the path to the dumped file on success or returns an error on failure.

#### Method `update_main_logger`
* `update_main_logger(config)`
    * `config`: [`MainLoggerConfig`](#type-mainloggerconfig)
* result: `null`

Changes main logger config options while CKB is running.

#### Method `set_extra_logger`
* `set_extra_logger(name, config_opt)`
    * `name`: `string`
    * `config_opt`: [`ExtraLoggerConfig`](#type-extraloggerconfig) `|` `null`
* result: `null`

Sets logger config options for extra loggers.

CKB nodes allow setting up extra loggers. These loggers will have their own log files and they only append logs to their log files.

##### Params

*   `name` - Extra logger name

*   `config_opt` - Adds a new logger or update an existing logger when this is not null. Removes the logger when this is null.

### Module Experiment

RPC Module Experiment for experimenting methods.

**EXPERIMENTAL warning**

The methods here may be removed or changed in future releases without prior notifications.

#### Method `compute_transaction_hash`
* `compute_transaction_hash(tx)`
    * `tx`: [`Transaction`](#type-transaction)
* result: [`H256`](#type-h256)

👎 Deprecated since 0.36.0:
Please implement molecule and compute the transaction hash in clients.


#### Method `compute_script_hash`
* `compute_script_hash(script)`
    * `script`: [`Script`](#type-script)
* result: [`H256`](#type-h256)

👎 Deprecated since 0.36.0:
Please implement molecule and compute the script hash in clients.


#### Method `dry_run_transaction`
* `dry_run_transaction(tx)`
    * `tx`: [`Transaction`](#type-transaction)
* result: [`DryRunResult`](#type-dryrunresult)

Dry run transaction and return the execution cycles.

This method will not check the transaction validity, but only run the lock script and type script and then return the execution cycles.

It is used to debug transaction scripts and query how many cycles the scripts consume.

##### Errors

*   [`TransactionFailedToResolve (-301)`](#error-transactionfailedtoresolve) - Failed to resolve the referenced cells and headers used in the transaction, as inputs or dependencies.

*   [`TransactionFailedToVerify (-302)`](#error-transactionfailedtoverify) - There is a script returns with an error.

##### Examples

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
            "args": "0x",
            "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
            "hash_type": "data"
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
* `calculate_dao_maximum_withdraw(out_point, block_hash)`
    * `out_point`: [`OutPoint`](#type-outpoint)
    * `block_hash`: [`H256`](#type-h256)
* result: [`Capacity`](#type-capacity)

Calculates the maximum withdraw one can get, given a referenced DAO cell, and a withdraw block hash.

##### Params

*   `out_point` - Reference to the DAO cell.

*   `block_hash` - The assumed reference block for withdraw. This block must be in the[canonical chain]('trait.ChainRpc.html#canonical-chain').

##### Returns

The RPC returns the final capacity when the cell `out_point` is withdrawn using the block`block_hash` as the reference.

In CKB, scripts cannot get the information about in which block the transaction is committed. A workaround is letting the transaction reference a block hash so the script knows that the transaction is committed at least after the reference block.

##### Errors

*   [`DaoError (-5)`](#error-daoerror) - The given out point is not a valid cell for DAO computation.

*   [`CKBInternalError (-1)`](#error-ckbinternalerror) - Mathematics overflow.

##### Examples

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

#### Method `estimate_fee_rate`
* `estimate_fee_rate(expect_confirm_blocks)`
    * `expect_confirm_blocks`: [`Uint64`](#type-uint64)
* result: [`EstimateResult`](#type-estimateresult)

👎 Deprecated since 0.34.0:
This method is deprecated because of the performance issue. It always returns an error now.


### Module Indexer

RPC Module Indexer which index cells by lock script hash.

The index is disabled by default, which **must** be enabled by calling [`index_lock_hash`](#method-index_lock_hash) first.

#### Method `get_live_cells_by_lock_hash`
* `get_live_cells_by_lock_hash(lock_hash, page, per_page, reverse_order)`
    * `lock_hash`: [`H256`](#type-h256)
    * `page`: [`Uint64`](#type-uint64)
    * `per_page`: [`Uint64`](#type-uint64)
    * `reverse_order`: `boolean` `|` `null`
* result: `Array<`[`LiveCell`](#type-livecell)`>`

👎 Deprecated since 0.36.0:
Please use [ckb-indexer](https://github.com/nervosnetwork/ckb-indexer) as an alternate solution.


#### Method `get_transactions_by_lock_hash`
* `get_transactions_by_lock_hash(lock_hash, page, per_page, reverse_order)`
    * `lock_hash`: [`H256`](#type-h256)
    * `page`: [`Uint64`](#type-uint64)
    * `per_page`: [`Uint64`](#type-uint64)
    * `reverse_order`: `boolean` `|` `null`
* result: `Array<`[`CellTransaction`](#type-celltransaction)`>`

👎 Deprecated since 0.36.0:
Please use [ckb-indexer](https://github.com/nervosnetwork/ckb-indexer) as an alternate solution.


#### Method `index_lock_hash`
* `index_lock_hash(lock_hash, index_from)`
    * `lock_hash`: [`H256`](#type-h256)
    * `index_from`: [`BlockNumber`](#type-blocknumber) `|` `null`
* result: [`LockHashIndexState`](#type-lockhashindexstate)

👎 Deprecated since 0.36.0:
Please use [ckb-indexer](https://github.com/nervosnetwork/ckb-indexer) as an alternate solution.


#### Method `deindex_lock_hash`
* `deindex_lock_hash(lock_hash)`
    * `lock_hash`: [`H256`](#type-h256)
* result: `null`

👎 Deprecated since 0.36.0:
Please use [ckb-indexer](https://github.com/nervosnetwork/ckb-indexer) as an alternate solution.


#### Method `get_lock_hash_index_states`
* `get_lock_hash_index_states()`
* result: `Array<`[`LockHashIndexState`](#type-lockhashindexstate)`>`

👎 Deprecated since 0.36.0:
Please use [ckb-indexer](https://github.com/nervosnetwork/ckb-indexer) as an alternate solution.


#### Method `get_capacity_by_lock_hash`
* `get_capacity_by_lock_hash(lock_hash)`
    * `lock_hash`: [`H256`](#type-h256)
* result: [`LockHashCapacity`](#type-lockhashcapacity) `|` `null`

👎 Deprecated since 0.36.0:
Please use [ckb-indexer](https://github.com/nervosnetwork/ckb-indexer) as an alternate solution.


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

Miners can assemble the new block from the template. The RPC is designed to allow miners removing transactions and adding new transactions to the block.

##### Params

*   `bytes_limit` - the max serialization size in bytes of the block. (**Optional:** the default is the consensus limit.)

*   `proposals_limit` - the max count of proposals. (**Optional:** the default is the consensus limit.)

*   `max_version` - the max block version. (**Optional:** the default is one configured in the current client version.)

##### Examples

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
              "args": "0x",
              "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
              "hash_type": "data"
            },
            "type": null
          }
        ],
        "outputs_data": [
          "0x"
        ],
        "version": "0x0",
        "witnesses": [
          "0x590000000c00000055000000490000001000000030000000310000001892ea40d82b53c678ff88312450bbb17e164d7a3e0a90941aa58839f56f8df20114000000b2e61ff569acf041b3c2c17724e2379c581eeac300000000"
        ]
      },
      "hash": "0xbaf7e4db2fd002f19a597ca1a31dfe8cfe26ed8cebc91f52b75b16a7a5ec8bab"
    },
    "compact_target": "0x1e083126",
    "current_time": "0x174c45e17a3",
    "cycles_limit": "0xd09dc300",
    "dao": "0xd495a106684401001e47c0ae1d5930009449d26e32380000000721efd0030000",
    "epoch": "0x7080019000001",
    "number": "0x401",
    "parent_hash": "0xa5f5c85987a15de25661e5a214f2c1449cd803f071acc7999820f25246471f40",
    "proposals": ["0xa0ef4eb5f4ceeb08a4c8"],
    "transactions": [],
    "uncles": [],
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

Submit new block to network

##### Params

*   `work_id` - The same work ID returned from [`get_block_template`](#method-get_block_template).

*   `block` - The assembed block from the block template and which PoW puzzle has been resolved.

##### Examples

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
        "nonce": "0x0",
        "number": "0x400",
        "parent_hash": "0xae003585fa15309b30b31aed3dcf385e9472c3c3e93746a6c4540629a6a1ed2d",
        "proposals_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
        "timestamp": "0x5cd2b117",
        "transactions_root": "0xc47d5b78b3c4c4c853e2a32810818940d0ee403423bea9ec7b8e566d9595206c",
        "uncles_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
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
                "args": "0x",
                "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
                "hash_type": "data"
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

##### Examples

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
* result: `Array<`[`RemoteNode`](#type-remotenode)`>`

Returns the connected peers information.

##### Examples

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
* result: `Array<`[`BannedAddr`](#type-bannedaddr)`>`

Returns all banned IPs/Subnets.

##### Examples

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

##### Examples

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

##### Params

*   `address` - The IP/Subnet with an optional netmask (default is /32 = single IP). Examples:
    *   "192.168.0.2" bans a single IP

    *   "192.168.0.0/24" bans IP from "192.168.0.0" to "192.168.0.255".


*   `command` - `insert` to insert an IP/Subnet to the list, `delete` to delete an IP/Subnet from the list.

*   `ban_time` - Time in milliseconds how long (or until when if [absolute] is set) the IP is banned, optional parameter, null means using the default time of 24h

*   `absolute` - If set, the `ban_time` must be an absolute timestamp in milliseconds since epoch, optional parameter.

*   `reason` - Ban reason, optional parameter.

##### Errors

*   [`InvalidParams (-32602)`](#error-invalidparams)
    *   Expected `address` to be a valid IP address with an optional netmask.

    *   Expected `command` to be in the list [insert, delete].


##### Examples

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

##### Examples

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

##### Params

*   `state` - true to enable networking, false to disable

##### Examples

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

##### Params

*   `peer_id` - The node id of node.

*   `address` - The address of node

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

##### Examples

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

##### Params

*   `peer_id` - The peer id of node.

This is the last part of a full P2P address. For example, in address "/ip4/192.168.2.100/tcp/8114/QmUsZHPbjjzU627UZFt4k8j6ycEcNvXRnVGxCPKqwbAfQS", the `peer_id` is `QmUsZHPbjjzU627UZFt4k8j6ycEcNvXRnVGxCPKqwbAfQS`.

##### Examples

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

Requests that a ping be sent to all connected peers, to measure ping time.

##### Examples

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

Submits a new transaction into the transaction pool.

##### Params

*   `transaction` - The transaction.

*   `outputs_validator` - Validates the transaction outputs before entering the tx-pool. (**Optional**, default is "passthrough").

##### Errors

*   [`PoolRejectedTransactionByOutputsValidator (-1102)`](#error-poolrejectedtransactionbyoutputsvalidator) - The transaction is rejected by the validator specified by `outputs_validator`. If you really want to send transactions with advanced scripts, please set `outputs_validator` to "passthrough".

*   [`PoolRejectedTransactionByIllTransactionChecker (-1103)`](#error-poolrejectedtransactionbyilltransactionchecker) - Pool rejects some transactions which seem contain invalid VM instructions. See the issue link in the error message for details.

*   [`PoolRejectedTransactionByMinFeeRate (-1104)`](#error-poolrejectedtransactionbyminfeerate) - The transaction fee rate must be greater than or equal to the config option `tx_pool.min_fee_rate`.

*   [`PoolRejectedTransactionByMaxAncestorsCountLimit (-1105)`](#error-poolrejectedtransactionbymaxancestorscountlimit) - The ancestors count must be greater than or equal to the config option `tx_pool.max_ancestors_count`.

*   [`PoolIsFull (-1106)`](#error-poolisfull) - Pool is full.

*   [`PoolRejectedDuplicatedTransaction (-1107)`](#error-poolrejectedduplicatedtransaction) - The transaction is already in the pool.

*   [`TransactionFailedToResolve (-301)`](#error-transactionfailedtoresolve) - Failed to resolve the referenced cells and headers used in the transaction, as inputs or dependencies.

*   [`TransactionFailedToVerify (-302)`](#error-transactionfailedtoverify) - Failed to verify the transaction.

##### Examples

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
            "args": "0x",
            "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
            "hash_type": "data"
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

#### Method `tx_pool_info`
* `tx_pool_info()`
* result: [`TxPoolInfo`](#type-txpoolinfo)

Returns the transaction pool information.

##### Examples

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

##### Examples

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

### Module Stats

RPC Module Stats for getting various statistic data.

#### Method `get_blockchain_info`
* `get_blockchain_info()`
* result: [`ChainInfo`](#type-chaininfo)

Returns statistics about the chain.

##### Examples

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

#### Method `get_peers_state`
* `get_peers_state()`
* result: `Array<`[`PeerState`](#type-peerstate)`>`

👎 Deprecated since 0.12.0:
Please use RPC [`get_peers`](#method-get_peers) instead


### Module Subscription

RPC Module Subscription that CKB node will push new messages to subscribers.

RPC subscriptions require a full duplex connection. CKB offers such connections in the form of TCP (enable with rpc.tcp_listen_address configuration option) and WebSocket (enable with rpc.ws_listen_address).

#### Examples

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

##### Params

*   `topic` - Subscription topic (enum: new_tip_header | new_tip_block)

##### Returns

This RPC returns the subscription ID as the result. CKB node will push messages in the subscribed topics to current RPC connection. The subscript ID is also attached as`params.subscription` in the push messages.

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

##### Topics

###### `new_tip_header`

Whenever there's a block is appended to the canonical chain, CKB node will publish the block header to subscribers.

The type of the `params.result` in the push message is [`HeaderView`](../../ckb_jsonrpc_types/struct.HeaderView.html).

###### `new_tip_block`

Whenever there's a block is appended to the canonical chain, CKB node will publish the whole block to subscribers.

The type of the `params.result` in the push message is [`BlockView`](../../ckb_jsonrpc_types/struct.BlockView.html).

###### `new_transaction`

Subscriber will get notified when new transaction is submitted to pool.

The type of the `params.result` in the push message is [`PoolTransactionEntry`](../../ckb_jsonrpc_types/struct.PoolTransactionEntry.html).

##### Examples

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

##### Params

*   `id` - Subscription ID

##### Examples

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

CKB application defined errors follow some patterns to assign the codes:

*   -1 ~ -999 are general errors

*   -1000 ~ -2999 are module specific errors. Each module generally gets 100 reserved error codes.

Unless otherwise noted, all the errors returns optional detailed information as `string` in the error object `data` field.

### Error `CKBInternalError`

(-1): CKB internal errors which are considered to never happen or only happen when the system resources are exhausted.

### Error `Deprecated`

(-2): The CKB method has been deprecated and disabled.

Set `rpc.enable_deprecated_rpc` to `true` in the config file to enable all deprecated methods.

### Error `Invalid`

(-3): Error code -3 is no longer used.

Prior to v0.35.0, CKB returns all RPC errors using the error code -3. CKB no longer uses -3 since v0.35.0.

### Error `RPCModuleIsDisabled`

(-4): The RPC method is not enabled.

CKB groups RPC methods into modules, and a method is enabled only when the module is explicitly enabled in the config file.

### Error `DaoError`

(-5): DAO related errors.

### Error `IntegerOverflow`

(-6): Integer operation overflow.

### Error `ConfigError`

(-7): The erorr is caused by a config file option.

User has to edit the config file to fix the error.

### Error `P2PFailedToBroadcast`

(-101): The CKB local node failed to broadcast a message to its peers.

### Error `DatabaseError`

(-200): Internal database error.

The CKB node persists data to the database. This is the error from the underlying database module.

### Error `ChainIndexIsInconsistent`

(-201): The chain index is inconsistent.

An example of inconsistent index is that the chain index says a block hash is in the chain but the block cannot be read from the database.

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


## RPC Types
