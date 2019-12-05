# CKB JSON-RPC Protocols

NOTE: This file is auto-generated. Please don't update this file directly; instead make changes to `rpc/json/rpc.json` and re-run `make gen-rpc-doc`

The RPC interface shares the version of the node version, which is returned in `local_node_info`. The interface is fully compactible between patch versions, for example, a client for 0.25.0 should work with 0.25.x for any x.

Allowing arbitrary machines to access the JSON-RPC port (using the `rpc.listen_address` configuration option) is **dangerous and strongly discouraged**. Please strictly limit the access to only trusted machines.

CKB JSON-RPC only supports HTTP now. If you need SSL, please setup a proxy via Nginx or other HTTP servers.


*   [`Chain`](#chain)
    *   [`get_tip_block_number`](#get_tip_block_number)
    *   [`get_tip_header`](#get_tip_header)
    *   [`get_current_epoch`](#get_current_epoch)
    *   [`get_epoch_by_number`](#get_epoch_by_number)
    *   [`get_block_hash`](#get_block_hash)
    *   [`get_block`](#get_block)
    *   [`get_header`](#get_header)
    *   [`get_header_by_number`](#get_header_by_number)
    *   [`get_cells_by_lock_hash`](#get_cells_by_lock_hash)
    *   [`get_live_cell`](#get_live_cell)
    *   [`get_transaction`](#get_transaction)
    *   [`get_cellbase_output_capacity_details`](#get_cellbase_output_capacity_details)
    *   [`get_block_by_number`](#get_block_by_number)
*   [`Experiment`](#experiment)
    *   [`dry_run_transaction`](#dry_run_transaction)
    *   [`_compute_transaction_hash`](#_compute_transaction_hash)
    *   [`calculate_dao_maximum_withdraw`](#calculate_dao_maximum_withdraw)
    *   [`estimate_fee_rate`](#estimate_fee_rate)
    *   [`_compute_script_hash`](#_compute_script_hash)
*   [`Indexer`](#indexer)
    *   [`index_lock_hash`](#index_lock_hash)
    *   [`get_lock_hash_index_states`](#get_lock_hash_index_states)
    *   [`get_live_cells_by_lock_hash`](#get_live_cells_by_lock_hash)
    *   [`get_transactions_by_lock_hash`](#get_transactions_by_lock_hash)
    *   [`deindex_lock_hash`](#deindex_lock_hash)
*   [`Miner`](#miner)
    *   [`get_block_template`](#get_block_template)
    *   [`submit_block`](#submit_block)
*   [`Net`](#net)
    *   [`local_node_info`](#local_node_info)
    *   [`get_peers`](#get_peers)
    *   [`get_banned_addresses`](#get_banned_addresses)
    *   [`set_ban`](#set_ban)
*   [`Pool`](#pool)
    *   [`send_transaction`](#send_transaction)
    *   [`tx_pool_info`](#tx_pool_info)
*   [`Stats`](#stats)
    *   [`get_blockchain_info`](#get_blockchain_info)
    *   [`get_peers_state`](#get_peers_state)

## Chain

### `get_tip_block_number`

Returns the number of blocks in the longest blockchain.


#### Examples

```bash
echo '{
    "id": 2,
    "jsonrpc": "2.0",
    "method": "get_tip_block_number",
    "params": []
}' \
| tr -d '\n' \
| curl -H 'content-type: application/json' -d @- \
http://localhost:8114
```

```json
{
    "id": 2,
    "jsonrpc": "2.0",
    "result": "0x400"
}
```

### `get_tip_header`

Returns the information about the tip header of the longest.


#### Examples

```bash
echo '{
    "id": 2,
    "jsonrpc": "2.0",
    "method": "get_tip_header",
    "params": []
}' \
| tr -d '\n' \
| curl -H 'content-type: application/json' -d @- \
http://localhost:8114
```

```json
{
    "id": 2,
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

### `get_current_epoch`

Returns the information about the current epoch.


#### Examples

```bash
echo '{
    "id": 2,
    "jsonrpc": "2.0",
    "method": "get_current_epoch",
    "params": []
}' \
| tr -d '\n' \
| curl -H 'content-type: application/json' -d @- \
http://localhost:8114
```

```json
{
    "id": 2,
    "jsonrpc": "2.0",
    "result": {
        "compact_target": "0x1e083126",
        "length": "0x708",
        "number": "0x1",
        "start_number": "0x3e8"
    }
}
```

### `get_epoch_by_number`

Return the information corresponding the given epoch number.

#### Parameters

    epoch_number - Epoch number

#### Examples

```bash
echo '{
    "id": 2,
    "jsonrpc": "2.0",
    "method": "get_epoch_by_number",
    "params": [
        "0x0"
    ]
}' \
| tr -d '\n' \
| curl -H 'content-type: application/json' -d @- \
http://localhost:8114
```

```json
{
    "id": 2,
    "jsonrpc": "2.0",
    "result": {
        "compact_target": "0x20010000",
        "length": "0x3e8",
        "number": "0x0",
        "start_number": "0x0"
    }
}
```

### `get_block_hash`

Returns the hash of a block in the best-block-chain by block number; block of No.0 is the genesis block.

#### Parameters

    block_number - Number of a block

#### Examples

```bash
echo '{
    "id": 2,
    "jsonrpc": "2.0",
    "method": "get_block_hash",
    "params": [
        "0x400"
    ]
}' \
| tr -d '\n' \
| curl -H 'content-type: application/json' -d @- \
http://localhost:8114
```

```json
{
    "id": 2,
    "jsonrpc": "2.0",
    "result": "0xa5f5c85987a15de25661e5a214f2c1449cd803f071acc7999820f25246471f40"
}
```

### `get_block`

Returns the information about a block by hash.

#### Parameters

    hash - Hash of a block

#### Examples

```bash
echo '{
    "id": 2,
    "jsonrpc": "2.0",
    "method": "get_block",
    "params": [
        "0xa5f5c85987a15de25661e5a214f2c1449cd803f071acc7999820f25246471f40"
    ]
}' \
| tr -d '\n' \
| curl -H 'content-type: application/json' -d @- \
http://localhost:8114
```

```json
{
    "id": 2,
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

### `get_header`

Returns the information about a block header by hash.


#### Examples

```bash
echo '{
    "id": 2,
    "jsonrpc": "2.0",
    "method": "get_header",
    "params": [
        "0xa5f5c85987a15de25661e5a214f2c1449cd803f071acc7999820f25246471f40"
    ]
}' \
| tr -d '\n' \
| curl -H 'content-type: application/json' -d @- \
http://localhost:8114
```

```json
{
    "id": 2,
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

### `get_header_by_number`

Returns the information about a block header by block number.


#### Examples

```bash
echo '{
    "id": 2,
    "jsonrpc": "2.0",
    "method": "get_header_by_number",
    "params": [
        "0x400"
    ]
}' \
| tr -d '\n' \
| curl -H 'content-type: application/json' -d @- \
http://localhost:8114
```

```json
{
    "id": 2,
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

### `get_cells_by_lock_hash`

Returns the information about live cells collection by the hash of lock script.

#### Parameters

    lock_hash - Cell lock script hash
    from - Start block number
    to - End block number
#### Returns

    block_hash - Refer to block
    capacity - Cell capacity
    cellbase - Cellbase or not
    lock - Cell lock script
    out_point - Refer to this output
    output_data_len - Corresponding output data length
    type - Cell type script

#### Examples

```bash
echo '{
    "id": 2,
    "jsonrpc": "2.0",
    "method": "get_cells_by_lock_hash",
    "params": [
        "0x4ceaa32f692948413e213ce6f3a83337145bde6e11fd8cb94377ce2637dcc412",
        "0xa",
        "0xe"
    ]
}' \
| tr -d '\n' \
| curl -H 'content-type: application/json' -d @- \
http://localhost:8114
```

```json
{
    "id": 2,
    "jsonrpc": "2.0",
    "result": [
        {
            "block_hash": "0xf293d02ce5e101b160912aaf15b1b87517b7a6d572c13af9ae4101c1143b22ad",
            "capacity": "0x2ca86f2642",
            "cellbase": true,
            "lock": {
                "args": "0x",
                "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
                "hash_type": "data"
            },
            "out_point": {
                "index": "0x0",
                "tx_hash": "0xa510932a80fda15a774203404453c5f9c0e8582f11c40f8ce5396f2460f8ccbf"
            },
            "output_data_len": "0x0",
            "type": null
        },
        {
            "block_hash": "0x63b872c02b1c2bd0c1af4f73f68ac04e2a3763a71f9656a823848d346619ffde",
            "capacity": "0x2ca86e3dd4",
            "cellbase": true,
            "lock": {
                "args": "0x",
                "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
                "hash_type": "data"
            },
            "out_point": {
                "index": "0x0",
                "tx_hash": "0x0b0fb337a9168132d3771f07e0ba055419c7e8f7bc2681a9eb445e61f44e1eb9"
            },
            "output_data_len": "0x0",
            "type": null
        },
        {
            "block_hash": "0x6bbdd9dc71784d500daadf391ca9035900b3ff18ed868d7d4fe4b17fdea88853",
            "capacity": "0x2ca86d5691",
            "cellbase": true,
            "lock": {
                "args": "0x",
                "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
                "hash_type": "data"
            },
            "out_point": {
                "index": "0x0",
                "tx_hash": "0xc336a23a785f3fec8b6e29e2c00d23483f1c6ad410b6b9fc0f62baf65d5efcc7"
            },
            "output_data_len": "0x0",
            "type": null
        }
    ]
}
```

### `get_live_cell`

Returns the information about a cell by out_point if it is live. If second with_data argument set to true, will return cell data and data_hash if it is live

#### Parameters

    out_point - OutPoint object {"tx_hash": <tx_hash>, "index": <index>}.
    with_data - Boolean

#### Examples

```bash
echo '{
    "id": 2,
    "jsonrpc": "2.0",
    "method": "get_live_cell",
    "params": [
        {
            "index": "0x0",
            "tx_hash": "0xa4037a893eb48e18ed4ef61034ce26eba9c585f15c9cee102ae58505565eccc3"
        },
        true
    ]
}' \
| tr -d '\n' \
| curl -H 'content-type: application/json' -d @- \
http://localhost:8114
```

```json
{
    "id": 2,
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

### `get_transaction`

Returns the information about a transaction requested by transaction hash.

#### Parameters

    hash - Hash of a transaction

#### Examples

```bash
echo '{
    "id": 2,
    "jsonrpc": "2.0",
    "method": "get_transaction",
    "params": [
        "0xa0ef4eb5f4ceeb08a4c8524d84c5da95dce2f608e0ca2ec8091191b0f330c6e3"
    ]
}' \
| tr -d '\n' \
| curl -H 'content-type: application/json' -d @- \
http://localhost:8114
```

```json
{
    "id": 2,
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

### `get_cellbase_output_capacity_details`

Returns each component of the created CKB in this block's cellbase, which is issued to a block N - 1 - ProposalWindow.farthest, where this block's height is N.

#### Parameters

    hash - Block hash

#### Examples

```bash
echo '{
    "id": 2,
    "jsonrpc": "2.0",
    "method": "get_cellbase_output_capacity_details",
    "params": [
        "0xa5f5c85987a15de25661e5a214f2c1449cd803f071acc7999820f25246471f40"
    ]
}' \
| tr -d '\n' \
| curl -H 'content-type: application/json' -d @- \
http://localhost:8114
```

```json
{
    "id": 2,
    "jsonrpc": "2.0",
    "result": {
        "primary": "0x18ce922bca",
        "proposal_reward": "0x0",
        "secondary": "0x17b93605",
        "total": "0x18e64b61cf",
        "tx_fee": "0x0"
    }
}
```

### `get_block_by_number`

Get block by number

#### Parameters

    block_number - Number of a block

#### Examples

```bash
echo '{
    "id": 2,
    "jsonrpc": "2.0",
    "method": "get_block_by_number",
    "params": [
        "0x400"
    ]
}' \
| tr -d '\n' \
| curl -H 'content-type: application/json' -d @- \
http://localhost:8114
```

```json
{
    "id": 2,
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

## Experiment

### `dry_run_transaction`

Dry run transaction and return the execution cycles.

This method will not check the transaction validity, but only run the lock script
and type script and then return the execution cycles.
Used to debug transaction scripts and query how many cycles the scripts consume


#### Examples

```bash
echo '{
    "id": 2,
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
}' \
| tr -d '\n' \
| curl -H 'content-type: application/json' -d @- \
http://localhost:8114
```

```json
{
    "id": 2,
    "jsonrpc": "2.0",
    "result": {
        "cycles": "0x219"
    }
}
```

### `_compute_transaction_hash`

Return the transaction hash

**Deprecated**: will be removed in a later version

#### Parameters

    transaction - The transaction object
    version - Transaction version
    cell_deps - Cell dependencies
    header_deps - Header dependencies
    inputs - Transaction inputs
    outputs - Transaction outputs
    witnesses - Witnesses

#### Examples

```bash
echo '{
    "id": 2,
    "jsonrpc": "2.0",
    "method": "_compute_transaction_hash",
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
}' \
| tr -d '\n' \
| curl -H 'content-type: application/json' -d @- \
http://localhost:8114
```

```json
{
    "id": 2,
    "jsonrpc": "2.0",
    "result": "0xa0ef4eb5f4ceeb08a4c8524d84c5da95dce2f608e0ca2ec8091191b0f330c6e3"
}
```

### `calculate_dao_maximum_withdraw`

Calculate the maximum withdraw one can get, given a referenced DAO cell, and a withdraw block hash

#### Parameters

    out_point - OutPoint object {"tx_hash": <tx_hash>, "index": <index>}.
    withdraw_block_hash - Block hash

#### Examples

```bash
echo '{
    "id": 2,
    "jsonrpc": "2.0",
    "method": "calculate_dao_maximum_withdraw",
    "params": [
        {
            "index": "0x0",
            "tx_hash": "0xa4037a893eb48e18ed4ef61034ce26eba9c585f15c9cee102ae58505565eccc3"
        },
        "0xa5f5c85987a15de25661e5a214f2c1449cd803f071acc7999820f25246471f40"
    ]
}' \
| tr -d '\n' \
| curl -H 'content-type: application/json' -d @- \
http://localhost:8114
```

```json
{
    "id": 2,
    "jsonrpc": "2.0",
    "result": "0x4a8b4e8a4"
}
```

### `estimate_fee_rate`

Estimate a fee rate (capacity/KB) for a transaction that to be committed in expect blocks.

This method estimate fee rate by sample transactions that collected from p2p network
expected_confirm_blocks must be between 3 and 1000
an error will return if samples is not enough


#### Examples

```bash
echo '{
    "id": 2,
    "jsonrpc": "2.0",
    "method": "estimate_fee_rate",
    "params": [
        "0xa"
    ]
}' \
| tr -d '\n' \
| curl -H 'content-type: application/json' -d @- \
http://localhost:8114
```

```json
{
    "id": 2,
    "jsonrpc": "2.0",
    "result": {
        "fee_rate": "0x7d0"
    }
}
```

### `_compute_script_hash`

Returns script hash of given transaction script

**Deprecated**: will be removed in a later version

#### Parameters

    args - Hex encoded arguments passed to reference cell
    code_hash - Code hash of referenced cell
    hash_type - data: code_hash matches against dep cell data hash; type: code_hash matches against dep cell type hash.

#### Examples

```bash
echo '{
    "id": 2,
    "jsonrpc": "2.0",
    "method": "_compute_script_hash",
    "params": [
        {
            "args": "0x",
            "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
            "hash_type": "data"
        }
    ]
}' \
| tr -d '\n' \
| curl -H 'content-type: application/json' -d @- \
http://localhost:8114
```

```json
{
    "id": 2,
    "jsonrpc": "2.0",
    "result": "0x4ceaa32f692948413e213ce6f3a83337145bde6e11fd8cb94377ce2637dcc412"
}
```

## Indexer

### `index_lock_hash`

Create index for live cells and transactions by the hash of lock script.

#### Parameters

    lock_hash - Cell lock script hash
    index_from - Create an index from starting block number (exclusive), an optional parameter, null means starting from tip and 0 means starting from genesis

#### Examples

```bash
echo '{
    "id": 2,
    "jsonrpc": "2.0",
    "method": "index_lock_hash",
    "params": [
        "0x4ceaa32f692948413e213ce6f3a83337145bde6e11fd8cb94377ce2637dcc412",
        "0x400"
    ]
}' \
| tr -d '\n' \
| curl -H 'content-type: application/json' -d @- \
http://localhost:8114
```

```json
{
    "id": 2,
    "jsonrpc": "2.0",
    "result": {
        "block_hash": "0xa5f5c85987a15de25661e5a214f2c1449cd803f071acc7999820f25246471f40",
        "block_number": "0x400",
        "lock_hash": "0x4ceaa32f692948413e213ce6f3a83337145bde6e11fd8cb94377ce2637dcc412"
    }
}
```

### `get_lock_hash_index_states`

Get lock hash index states


#### Examples

```bash
echo '{
    "id": 2,
    "jsonrpc": "2.0",
    "method": "get_lock_hash_index_states",
    "params": []
}' \
| tr -d '\n' \
| curl -H 'content-type: application/json' -d @- \
http://localhost:8114
```

```json
{
    "id": 2,
    "jsonrpc": "2.0",
    "result": [
        {
            "block_hash": "0xa5f5c85987a15de25661e5a214f2c1449cd803f071acc7999820f25246471f40",
            "block_number": "0x400",
            "lock_hash": "0x4ceaa32f692948413e213ce6f3a83337145bde6e11fd8cb94377ce2637dcc412"
        }
    ]
}
```

### `get_live_cells_by_lock_hash`

Returns the live cells collection by the hash of lock script.

#### Parameters

    lock_hash - Cell lock script hash
    page - Page number, starts from 0
    per - Page size, max value is 50
    reverse_order - Returns the live cells collection in reverse order, an optional parameter, default is false
#### Returns

    cell_output - Cell output struct
    cellbase - Cellbase or not
    created_by - Refer to the transaction which creates this cell output
    output_data_len - Corresponding output data length

#### Examples

```bash
echo '{
    "id": 2,
    "jsonrpc": "2.0",
    "method": "get_live_cells_by_lock_hash",
    "params": [
        "0x4ceaa32f692948413e213ce6f3a83337145bde6e11fd8cb94377ce2637dcc412",
        "0xa",
        "0xe"
    ]
}' \
| tr -d '\n' \
| curl -H 'content-type: application/json' -d @- \
http://localhost:8114
```

```json
{
    "id": 2,
    "jsonrpc": "2.0",
    "result": [
        {
            "cell_output": {
                "capacity": "0x2cb6562e4e",
                "lock": {
                    "args": "0x",
                    "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
                    "hash_type": "data"
                },
                "type": null
            },
            "cellbase": true,
            "created_by": {
                "block_number": "0x98",
                "index": "0x0",
                "tx_hash": "0x2d811f9ad7f2f7319171a6da4c842dd78e36682b4ac74da4f67b97c9f7d7a02b"
            },
            "output_data_len": "0x0"
        },
        {
            "cell_output": {
                "capacity": "0x2cb66b2496",
                "lock": {
                    "args": "0x",
                    "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
                    "hash_type": "data"
                },
                "type": null
            },
            "cellbase": true,
            "created_by": {
                "block_number": "0x99",
                "index": "0x0",
                "tx_hash": "0x1ccf68bf7cb96a1a7f992c27bcfea6ebfc0fe32602196569aaa0cb3cd3e9f5ea"
            },
            "output_data_len": "0x0"
        },
        {
            "cell_output": {
                "capacity": "0x2cb68006e8",
                "lock": {
                    "args": "0x",
                    "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
                    "hash_type": "data"
                },
                "type": null
            },
            "cellbase": true,
            "created_by": {
                "block_number": "0x9a",
                "index": "0x0",
                "tx_hash": "0x74db38ad40184dd0528f4841e10599ff97bfbf2b5313754d1e96920d8523a5d4"
            },
            "output_data_len": "0x0"
        },
        {
            "cell_output": {
                "capacity": "0x2cb694d55e",
                "lock": {
                    "args": "0x",
                    "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
                    "hash_type": "data"
                },
                "type": null
            },
            "cellbase": true,
            "created_by": {
                "block_number": "0x9b",
                "index": "0x0",
                "tx_hash": "0xf7d0ecc70015b46c5ab1cc8462592ae612fdaada200f643f3e1ce633bcc5ad1d"
            },
            "output_data_len": "0x0"
        },
        {
            "cell_output": {
                "capacity": "0x2cb6a99016",
                "lock": {
                    "args": "0x",
                    "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
                    "hash_type": "data"
                },
                "type": null
            },
            "cellbase": true,
            "created_by": {
                "block_number": "0x9c",
                "index": "0x0",
                "tx_hash": "0xc3d232bb6b0e5d9a71a0978c9ab66c7a127ed37aeed6a2509dcc10d994c8c605"
            },
            "output_data_len": "0x0"
        },
        {
            "cell_output": {
                "capacity": "0x2cb6be372c",
                "lock": {
                    "args": "0x",
                    "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
                    "hash_type": "data"
                },
                "type": null
            },
            "cellbase": true,
            "created_by": {
                "block_number": "0x9d",
                "index": "0x0",
                "tx_hash": "0x10139a08beae170a35fbfcece6d50561ec61e13e4c6438435c1f2021331d7c4d"
            },
            "output_data_len": "0x0"
        },
        {
            "cell_output": {
                "capacity": "0x2cb6d2cabb",
                "lock": {
                    "args": "0x",
                    "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
                    "hash_type": "data"
                },
                "type": null
            },
            "cellbase": true,
            "created_by": {
                "block_number": "0x9e",
                "index": "0x0",
                "tx_hash": "0x39a083a1deb39b923a600a6f0714663085b5d2011b886b160962e20f1a28b550"
            },
            "output_data_len": "0x0"
        },
        {
            "cell_output": {
                "capacity": "0x2cb6e74ae0",
                "lock": {
                    "args": "0x",
                    "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
                    "hash_type": "data"
                },
                "type": null
            },
            "cellbase": true,
            "created_by": {
                "block_number": "0x9f",
                "index": "0x0",
                "tx_hash": "0x2899c066f80a04b9a168e4499760ad1d768f44a3d673779905d88edd86362ac6"
            },
            "output_data_len": "0x0"
        },
        {
            "cell_output": {
                "capacity": "0x2cb6fbb7b4",
                "lock": {
                    "args": "0x",
                    "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
                    "hash_type": "data"
                },
                "type": null
            },
            "cellbase": true,
            "created_by": {
                "block_number": "0xa0",
                "index": "0x0",
                "tx_hash": "0xe2579280875a5d14538b0cc2356707792189662d5f8292541d9856ef291e81bf"
            },
            "output_data_len": "0x0"
        },
        {
            "cell_output": {
                "capacity": "0x2cb7101155",
                "lock": {
                    "args": "0x",
                    "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
                    "hash_type": "data"
                },
                "type": null
            },
            "cellbase": true,
            "created_by": {
                "block_number": "0xa1",
                "index": "0x0",
                "tx_hash": "0xd6121e80237c79182d55ec0efb9fa75bc9cc592f818057ced51aac6bb625e016"
            },
            "output_data_len": "0x0"
        },
        {
            "cell_output": {
                "capacity": "0x2cb72457dc",
                "lock": {
                    "args": "0x",
                    "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
                    "hash_type": "data"
                },
                "type": null
            },
            "cellbase": true,
            "created_by": {
                "block_number": "0xa2",
                "index": "0x0",
                "tx_hash": "0x624eba1135e54a5988cb2ec70d42fa860d1d5658ed7f8d402615dff7d598e4b6"
            },
            "output_data_len": "0x0"
        },
        {
            "cell_output": {
                "capacity": "0x2cb7388b65",
                "lock": {
                    "args": "0x",
                    "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
                    "hash_type": "data"
                },
                "type": null
            },
            "cellbase": true,
            "created_by": {
                "block_number": "0xa3",
                "index": "0x0",
                "tx_hash": "0x7884b4cf85bc02cb73ec41d5cbbbf158eebca6ef855419ce57ff7c1d97b5be58"
            },
            "output_data_len": "0x0"
        },
        {
            "cell_output": {
                "capacity": "0x2cb74cac0a",
                "lock": {
                    "args": "0x",
                    "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
                    "hash_type": "data"
                },
                "type": null
            },
            "cellbase": true,
            "created_by": {
                "block_number": "0xa4",
                "index": "0x0",
                "tx_hash": "0xb613ba9b5f6177657493492dd523a63720d855ae9749887a0de881b894a1d6a6"
            },
            "output_data_len": "0x0"
        },
        {
            "cell_output": {
                "capacity": "0x2cb760b9e6",
                "lock": {
                    "args": "0x",
                    "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
                    "hash_type": "data"
                },
                "type": null
            },
            "cellbase": true,
            "created_by": {
                "block_number": "0xa5",
                "index": "0x0",
                "tx_hash": "0x701f4b962c1650810800ee6ed981841692c1939a4b597e9e7a726c5db77f6164"
            },
            "output_data_len": "0x0"
        }
    ]
}
```

### `get_transactions_by_lock_hash`

Returns the transactions collection by the hash of lock script. Returns empty array when the `lock_hash` has not been indexed yet.

#### Parameters

    lock_hash - Cell lock script hash
    page - Page number, starts from 0
    per - Page size, max value is 50
    reverse_order - Return the transactions collection in reverse order, an optional parameter, default is false

#### Examples

```bash
echo '{
    "id": 2,
    "jsonrpc": "2.0",
    "method": "get_transactions_by_lock_hash",
    "params": [
        "0x4ceaa32f692948413e213ce6f3a83337145bde6e11fd8cb94377ce2637dcc412",
        "0xa",
        "0xe"
    ]
}' \
| tr -d '\n' \
| curl -H 'content-type: application/json' -d @- \
http://localhost:8114
```

```json
{
    "id": 2,
    "jsonrpc": "2.0",
    "result": [
        {
            "consumed_by": null,
            "created_by": {
                "block_number": "0x98",
                "index": "0x0",
                "tx_hash": "0x2d811f9ad7f2f7319171a6da4c842dd78e36682b4ac74da4f67b97c9f7d7a02b"
            }
        },
        {
            "consumed_by": null,
            "created_by": {
                "block_number": "0x99",
                "index": "0x0",
                "tx_hash": "0x1ccf68bf7cb96a1a7f992c27bcfea6ebfc0fe32602196569aaa0cb3cd3e9f5ea"
            }
        },
        {
            "consumed_by": null,
            "created_by": {
                "block_number": "0x9a",
                "index": "0x0",
                "tx_hash": "0x74db38ad40184dd0528f4841e10599ff97bfbf2b5313754d1e96920d8523a5d4"
            }
        },
        {
            "consumed_by": null,
            "created_by": {
                "block_number": "0x9b",
                "index": "0x0",
                "tx_hash": "0xf7d0ecc70015b46c5ab1cc8462592ae612fdaada200f643f3e1ce633bcc5ad1d"
            }
        },
        {
            "consumed_by": null,
            "created_by": {
                "block_number": "0x9c",
                "index": "0x0",
                "tx_hash": "0xc3d232bb6b0e5d9a71a0978c9ab66c7a127ed37aeed6a2509dcc10d994c8c605"
            }
        },
        {
            "consumed_by": null,
            "created_by": {
                "block_number": "0x9d",
                "index": "0x0",
                "tx_hash": "0x10139a08beae170a35fbfcece6d50561ec61e13e4c6438435c1f2021331d7c4d"
            }
        },
        {
            "consumed_by": null,
            "created_by": {
                "block_number": "0x9e",
                "index": "0x0",
                "tx_hash": "0x39a083a1deb39b923a600a6f0714663085b5d2011b886b160962e20f1a28b550"
            }
        },
        {
            "consumed_by": null,
            "created_by": {
                "block_number": "0x9f",
                "index": "0x0",
                "tx_hash": "0x2899c066f80a04b9a168e4499760ad1d768f44a3d673779905d88edd86362ac6"
            }
        },
        {
            "consumed_by": null,
            "created_by": {
                "block_number": "0xa0",
                "index": "0x0",
                "tx_hash": "0xe2579280875a5d14538b0cc2356707792189662d5f8292541d9856ef291e81bf"
            }
        },
        {
            "consumed_by": null,
            "created_by": {
                "block_number": "0xa1",
                "index": "0x0",
                "tx_hash": "0xd6121e80237c79182d55ec0efb9fa75bc9cc592f818057ced51aac6bb625e016"
            }
        },
        {
            "consumed_by": null,
            "created_by": {
                "block_number": "0xa2",
                "index": "0x0",
                "tx_hash": "0x624eba1135e54a5988cb2ec70d42fa860d1d5658ed7f8d402615dff7d598e4b6"
            }
        },
        {
            "consumed_by": null,
            "created_by": {
                "block_number": "0xa3",
                "index": "0x0",
                "tx_hash": "0x7884b4cf85bc02cb73ec41d5cbbbf158eebca6ef855419ce57ff7c1d97b5be58"
            }
        },
        {
            "consumed_by": null,
            "created_by": {
                "block_number": "0xa4",
                "index": "0x0",
                "tx_hash": "0xb613ba9b5f6177657493492dd523a63720d855ae9749887a0de881b894a1d6a6"
            }
        },
        {
            "consumed_by": null,
            "created_by": {
                "block_number": "0xa5",
                "index": "0x0",
                "tx_hash": "0x701f4b962c1650810800ee6ed981841692c1939a4b597e9e7a726c5db77f6164"
            }
        }
    ]
}
```

### `deindex_lock_hash`

Remove index for live cells and transactions by the hash of lock script.

#### Parameters

    lock_hash - Cell lock script hash

#### Examples

```bash
echo '{
    "id": 2,
    "jsonrpc": "2.0",
    "method": "deindex_lock_hash",
    "params": [
        "0x4ceaa32f692948413e213ce6f3a83337145bde6e11fd8cb94377ce2637dcc412"
    ]
}' \
| tr -d '\n' \
| curl -H 'content-type: application/json' -d @- \
http://localhost:8114
```

```json
{
    "id": 2,
    "jsonrpc": "2.0",
    "result": null
}
```

## Miner

### `get_block_template`

Returns data needed to construct a block to work on

#### Parameters

    bytes_limit - optional number, specify the max bytes of block
    proposals_limit - optional number, specify the max proposals of block
    max_version - optional number, specify the max block version

#### Examples

```bash
echo '{
    "id": 2,
    "jsonrpc": "2.0",
    "method": "get_block_template",
    "params": [
        null,
        null,
        null
    ]
}' \
| tr -d '\n' \
| curl -H 'content-type: application/json' -d @- \
http://localhost:8114
```

```json
{
    "id": 2,
    "jsonrpc": "2.0",
    "result": {
        "bytes_limit": "0x22d387",
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
                        "since": "0x1"
                    }
                ],
                "outputs": [
                    {
                        "capacity": "0x1d1a94a200",
                        "lock": {
                            "args": [
                                "0xb2e61ff569acf041b3c2c17724e2379c581eeac3"
                            ],
                            "code_hash": "0x1892ea40d82b53c678ff88312450bbb17e164d7a3e0a90941aa58839f56f8df2",
                            "hash_type": "type"
                        },
                        "type": null
                    }
                ],
                "outputs_data": [
                    "0x"
                ],
                "version": "0x0",
                "witnesses": [
                    {
                        "data": [
                            "0x1892ea40d82b53c678ff88312450bbb17e164d7a3e0a90941aa58839f56f8df201",
                            "0x"
                        ]
                    }
                ]
            },
            "hash": "0x076049e2cc6b9f1ed4bb27b2337c55071dabfaf0183b1b17a4965bd0372d8dec"
        },
        "compact_target": "0x100",
        "current_time": "0x16d6269e84f",
        "cycles_limit": "0x2540be400",
        "dao": "0x004fb9e277860700b2f80165348723003d1862ec960000000028eb3d7e7a0100",
        "epoch": "0x3e80001000000",
        "number": "0x1",
        "parent_hash": "0xd5c495b7dd4d9d066a6a4d4356bc31955ad3199e0d856f34cfbe159c46ee335b",
        "proposals": [],
        "transactions": [],
        "uncles": [],
        "uncles_count_limit": "0x2",
        "version": "0x0",
        "work_id": "0x0"
    }
}
```

### `submit_block`

Submit new block to network

#### Parameters

    work_id - the identifier to proof-of-work
    block - new block

#### Examples

```bash
echo '{
    "id": 2,
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
}' \
| tr -d '\n' \
| curl -H 'content-type: application/json' -d @- \
http://localhost:8114
```

```json
{
    "id": 2,
    "jsonrpc": "2.0",
    "result": "0xa5f5c85987a15de25661e5a214f2c1449cd803f071acc7999820f25246471f40"
}
```

## Net

### `local_node_info`

Returns the local node information.


#### Examples

```bash
echo '{
    "id": 2,
    "jsonrpc": "2.0",
    "method": "local_node_info",
    "params": []
}' \
| tr -d '\n' \
| curl -H 'content-type: application/json' -d @- \
http://localhost:8114
```

```json
{
    "id": 2,
    "jsonrpc": "2.0",
    "result": {
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
        "is_outbound": null,
        "node_id": "QmTRHCdrRtgUzYLNCin69zEvPvLYdxUZLLfLYyHVY3DZAS",
        "version": "0.0.0"
    }
}
```

### `get_peers`

Returns the connected peers information.


#### Examples

```bash
echo '{
    "id": 2,
    "jsonrpc": "2.0",
    "method": "get_peers",
    "params": []
}' \
| tr -d '\n' \
| curl -H 'content-type: application/json' -d @- \
http://localhost:8114
```

```json
{
    "id": 2,
    "jsonrpc": "2.0",
    "result": [
        {
            "addresses": [
                {
                    "address": "/ip4/192.168.0.3/tcp/8115",
                    "score": "0x1"
                }
            ],
            "is_outbound": true,
            "node_id": "QmaaaLB4uPyDpZwTQGhV63zuYrKm4reyN2tF1j2ain4oE7",
            "version": "unknown"
        },
        {
            "addresses": [
                {
                    "address": "/ip4/192.168.0.4/tcp/8113",
                    "score": "0xff"
                }
            ],
            "is_outbound": false,
            "node_id": "QmRuGcpVC3vE7aEoB6fhUdq9uzdHbyweCnn1sDBSjfmcbM",
            "version": "unknown"
        },
        {
            "addresses": [],
            "node_id": "QmUddxwRqgTmT6tFujXbYPMLGLAE2Tciyv6uHGfdYFyDVa",
            "version": "unknown"
        }
    ]
}
```

### `get_banned_addresses`

Returns all banned IPs/Subnets.


#### Examples

```bash
echo '{
    "id": 2,
    "jsonrpc": "2.0",
    "method": "get_banned_addresses",
    "params": []
}' \
| tr -d '\n' \
| curl -H 'content-type: application/json' -d @- \
http://localhost:8114
```

```json
{
    "id": 2,
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

### `set_ban`

Insert or delete an IP/Subnet from the banned list

#### Parameters

    address - The IP/Subnet with an optional netmask (default is /32 = single IP)
    command - `insert` to insert an IP/Subnet to the list, `delete` to delete an IP/Subnet from the list
    ban_time - Time in milliseconds how long (or until when if [absolute] is set) the IP is banned, optional parameter, null means using the default time of 24h
    absolute - If set, the `ban_time` must be an absolute timestamp in milliseconds since epoch, optional parameter
    reason - Ban reason, optional parameter

#### Examples

```bash
echo '{
    "id": 2,
    "jsonrpc": "2.0",
    "method": "set_ban",
    "params": [
        "192.168.0.2",
        "insert",
        "0x1ac89236180",
        true,
        "set_ban example"
    ]
}' \
| tr -d '\n' \
| curl -H 'content-type: application/json' -d @- \
http://localhost:8114
```

```json
{
    "id": 2,
    "jsonrpc": "2.0",
    "result": null
}
```

## Pool

### `send_transaction`

Send new transaction into transaction pool

If <block_hash> of <previsous_output> is not specified, loads the corresponding input cell. If <block_hash> is specified, load the corresponding input cell only if the corresponding block exist and contain this cell as output.

#### Parameters

    transaction - The transaction object
    version - Transaction version
    cell_deps - Cell dependencies
    header_deps - Header dependencies
    inputs - Transaction inputs
    outputs - Transaction outputs
    witnesses - Witnesses

#### Examples

```bash
echo '{
    "id": 2,
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
        }
    ]
}' \
| tr -d '\n' \
| curl -H 'content-type: application/json' -d @- \
http://localhost:8114
```

```json
{
    "id": 2,
    "jsonrpc": "2.0",
    "result": "0xa0ef4eb5f4ceeb08a4c8524d84c5da95dce2f608e0ca2ec8091191b0f330c6e3"
}
```

### `tx_pool_info`

Return the transaction pool information


#### Examples

```bash
echo '{
    "id": 2,
    "jsonrpc": "2.0",
    "method": "tx_pool_info",
    "params": []
}' \
| tr -d '\n' \
| curl -H 'content-type: application/json' -d @- \
http://localhost:8114
```

```json
{
    "id": 2,
    "jsonrpc": "2.0",
    "result": {
        "last_txs_updated_at": "0x0",
        "orphan": "0x0",
        "pending": "0x1",
        "proposed": "0x0",
        "total_tx_cycles": "0x219",
        "total_tx_size": "0x112"
    }
}
```

## Stats

### `get_blockchain_info`

Return state info of blockchain


#### Examples

```bash
echo '{
    "id": 2,
    "jsonrpc": "2.0",
    "method": "get_blockchain_info",
    "params": []
}' \
| tr -d '\n' \
| curl -H 'content-type: application/json' -d @- \
http://localhost:8114
```

```json
{
    "id": 2,
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
        "chain": "main",
        "difficulty": "0x1f4003",
        "epoch": "0x7080018000001",
        "is_initial_block_download": true,
        "median_time": "0x5cd2b105"
    }
}
```

### `get_peers_state`

Deprecating in 0.12.0: Return state info of peers


#### Examples

```bash
echo '{
    "id": 2,
    "jsonrpc": "2.0",
    "method": "get_peers_state",
    "params": []
}' \
| tr -d '\n' \
| curl -H 'content-type: application/json' -d @- \
http://localhost:8114
```

```json
{
    "id": 2,
    "jsonrpc": "2.0",
    "result": [
        {
            "blocks_in_flight": "0x56",
            "last_updated": "0x16a95af332d",
            "peer": "0x1"
        }
    ]
}
```
