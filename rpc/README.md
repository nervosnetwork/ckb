# CKB JSON-RPC Protocols


*   [`Chain`](#chain)
    *   [`get_block`](#get_block)
    *   [`get_block_by_number`](#get_block_by_number)
    *   [`get_block_hash`](#get_block_hash)
    *   [`get_cellbase_output_capacity_details`](#get_cellbase_output_capacity_details)
    *   [`get_cells_by_lock_hash`](#get_cells_by_lock_hash)
    *   [`get_current_epoch`](#get_current_epoch)
    *   [`get_epoch_by_number`](#get_epoch_by_number)
    *   [`get_header`](#get_header)
    *   [`get_header_by_number`](#get_header_by_number)
    *   [`get_live_cell`](#get_live_cell)
    *   [`get_tip_block_number`](#get_tip_block_number)
    *   [`get_tip_header`](#get_tip_header)
    *   [`get_transaction`](#get_transaction)
*   [`Experiment`](#experiment)
    *   [`_compute_script_hash`](#_compute_script_hash)
    *   [`_compute_transaction_hash`](#_compute_transaction_hash)
    *   [`dry_run_transaction`](#dry_run_transaction)
*   [`Indexer`](#indexer)
    *   [`deindex_lock_hash`](#deindex_lock_hash)
    *   [`get_live_cells_by_lock_hash`](#get_live_cells_by_lock_hash)
    *   [`get_lock_hash_index_states`](#get_lock_hash_index_states)
    *   [`get_transactions_by_lock_hash`](#get_transactions_by_lock_hash)
    *   [`index_lock_hash`](#index_lock_hash)
*   [`Net`](#net)
    *   [`get_banned_addresses`](#get_banned_addresses)
    *   [`get_peers`](#get_peers)
    *   [`local_node_info`](#local_node_info)
    *   [`set_ban`](#set_ban)
*   [`Pool`](#pool)
    *   [`send_transaction`](#send_transaction)
    *   [`tx_pool_info`](#tx_pool_info)
*   [`Stats`](#stats)
    *   [`get_blockchain_info`](#get_blockchain_info)
    *   [`get_peers_state`](#get_peers_state)

## Chain

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
        "0x0a663a245ae0b3545a24e4ec06066b9e9f0bd32e5fdbb9038ec15af59ef2593d"
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
            "dao": "0x010000000000000063ae2493990ab00000f0c890408900000061eb7ada030000",
            "difficulty": "0x3e8",
            "epoch": "0",
            "hash": "0x0a663a245ae0b3545a24e4ec06066b9e9f0bd32e5fdbb9038ec15af59ef2593d",
            "number": "1024",
            "parent_hash": "0x5eae6dfdac4b3099619a131975fd0be6da3c099eeaae8261182627a53548fc87",
            "proposals_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
            "seal": {
                "nonce": "0",
                "proof": "0x"
            },
            "timestamp": "1557311767",
            "transactions_root": "0x64a872caca3c7b671b64897230e88dec8800b594347d192661908a87f7fbcb9b",
            "uncles_count": "0",
            "uncles_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
            "version": "0",
            "witnesses_root": "0x688c1ff7df5b9b454bd76a031d40e13b4e462aeee5f8497d0f9f6f1dbd5d8111"
        },
        "proposals": [],
        "transactions": [
            {
                "deps": [],
                "hash": "0x64a872caca3c7b671b64897230e88dec8800b594347d192661908a87f7fbcb9b",
                "inputs": [
                    {
                        "previous_output": {
                            "block_hash": null,
                            "cell": null
                        },
                        "since": "1024"
                    }
                ],
                "outputs": [
                    {
                        "capacity": "101346505683",
                        "data_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
                        "lock": {
                            "args": [],
                            "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
                            "hash_type": "Data"
                        },
                        "type": null
                    }
                ],
                "outputs_data": [
                    "0x"
                ],
                "version": "0",
                "witnesses": [
                    {
                        "data": [
                            "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a500"
                        ]
                    }
                ]
            }
        ],
        "uncles": []
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
        "1024"
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
            "dao": "0x010000000000000063ae2493990ab00000f0c890408900000061eb7ada030000",
            "difficulty": "0x3e8",
            "epoch": "0",
            "hash": "0x0a663a245ae0b3545a24e4ec06066b9e9f0bd32e5fdbb9038ec15af59ef2593d",
            "number": "1024",
            "parent_hash": "0x5eae6dfdac4b3099619a131975fd0be6da3c099eeaae8261182627a53548fc87",
            "proposals_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
            "seal": {
                "nonce": "0",
                "proof": "0x"
            },
            "timestamp": "1557311767",
            "transactions_root": "0x64a872caca3c7b671b64897230e88dec8800b594347d192661908a87f7fbcb9b",
            "uncles_count": "0",
            "uncles_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
            "version": "0",
            "witnesses_root": "0x688c1ff7df5b9b454bd76a031d40e13b4e462aeee5f8497d0f9f6f1dbd5d8111"
        },
        "proposals": [],
        "transactions": [
            {
                "deps": [],
                "hash": "0x64a872caca3c7b671b64897230e88dec8800b594347d192661908a87f7fbcb9b",
                "inputs": [
                    {
                        "previous_output": {
                            "block_hash": null,
                            "cell": null
                        },
                        "since": "1024"
                    }
                ],
                "outputs": [
                    {
                        "capacity": "101346505683",
                        "data_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
                        "lock": {
                            "args": [],
                            "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
                            "hash_type": "Data"
                        },
                        "type": null
                    }
                ],
                "outputs_data": [
                    "0x"
                ],
                "version": "0",
                "witnesses": [
                    {
                        "data": [
                            "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a500"
                        ]
                    }
                ]
            }
        ],
        "uncles": []
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
        "1024"
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
    "result": "0x0a663a245ae0b3545a24e4ec06066b9e9f0bd32e5fdbb9038ec15af59ef2593d"
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
        "0x0a663a245ae0b3545a24e4ec06066b9e9f0bd32e5fdbb9038ec15af59ef2593d"
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
        "primary": "100000000000",
        "proposal_reward": "0",
        "secondary": "1346505683",
        "total": "101346505683",
        "tx_fee": "0"
    }
}
```

### `get_cells_by_lock_hash`

Returns the information about cells collection by the hash of lock script.

#### Parameters

    lock_hash - Cell lock script hash
    from - Start block number
    to - End block number

#### Examples

```bash
echo '{
    "id": 2,
    "jsonrpc": "2.0",
    "method": "get_cells_by_lock_hash",
    "params": [
        "0xa6ed2edad0d48a3d58d0bec407ddf2e40ddd5f533d7059a160149f4021c2a589",
        "0",
        "2"
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
            "capacity": "100000000000",
            "lock": {
                "args": [],
                "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
                "hash_type": "Data"
            },
            "out_point": {
                "block_hash": "0xa1e336d828ccbc65d92b9df191075275cfdaccaa3c2c7c8205a75920f44dc1e4",
                "cell": {
                    "index": "0",
                    "tx_hash": "0xb0a82422071490147538402b4cdb1ffad0d8810c24b5745aa7ff3580fc48bf8d"
                }
            }
        },
        {
            "capacity": "100000000000",
            "lock": {
                "args": [],
                "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
                "hash_type": "Data"
            },
            "out_point": {
                "block_hash": "0x610cb95fb75201174c3038f452c2e047368b20dd015be85897933cfaad4026d4",
                "cell": {
                    "index": "0",
                    "tx_hash": "0x641634c12815acea8518f3a2cb8e8385c5e14e126fa24c40d9222ffb760660ec"
                }
            }
        }
    ]
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
        "difficulty": "0x7a1200",
        "length": "1800",
        "number": "1",
        "start_number": "1000"
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
        "0"
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
        "difficulty": "0x3e8",
        "length": "1000",
        "number": "0",
        "start_number": "0"
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
        "0x0a663a245ae0b3545a24e4ec06066b9e9f0bd32e5fdbb9038ec15af59ef2593d"
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
        "dao": "0x010000000000000063ae2493990ab00000f0c890408900000061eb7ada030000",
        "difficulty": "0x3e8",
        "epoch": "0",
        "hash": "0x0a663a245ae0b3545a24e4ec06066b9e9f0bd32e5fdbb9038ec15af59ef2593d",
        "number": "1024",
        "parent_hash": "0x5eae6dfdac4b3099619a131975fd0be6da3c099eeaae8261182627a53548fc87",
        "proposals_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
        "seal": {
            "nonce": "0",
            "proof": "0x"
        },
        "timestamp": "1557311767",
        "transactions_root": "0x64a872caca3c7b671b64897230e88dec8800b594347d192661908a87f7fbcb9b",
        "uncles_count": "0",
        "uncles_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
        "version": "0",
        "witnesses_root": "0x688c1ff7df5b9b454bd76a031d40e13b4e462aeee5f8497d0f9f6f1dbd5d8111"
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
        "1024"
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
        "dao": "0x010000000000000063ae2493990ab00000f0c890408900000061eb7ada030000",
        "difficulty": "0x3e8",
        "epoch": "0",
        "hash": "0x0a663a245ae0b3545a24e4ec06066b9e9f0bd32e5fdbb9038ec15af59ef2593d",
        "number": "1024",
        "parent_hash": "0x5eae6dfdac4b3099619a131975fd0be6da3c099eeaae8261182627a53548fc87",
        "proposals_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
        "seal": {
            "nonce": "0",
            "proof": "0x"
        },
        "timestamp": "1557311767",
        "transactions_root": "0x64a872caca3c7b671b64897230e88dec8800b594347d192661908a87f7fbcb9b",
        "uncles_count": "0",
        "uncles_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
        "version": "0",
        "witnesses_root": "0x688c1ff7df5b9b454bd76a031d40e13b4e462aeee5f8497d0f9f6f1dbd5d8111"
    }
}
```

### `get_live_cell`

Returns the information about a cell by out_point. If <block_hash> is not specific, returns the cell if it is live. If <block_hash> is specified, return the live cell only if the corresponding block contain this cell

#### Parameters

    out_point - OutPoint object {{"tx_hash": <tx_hash>, "index": <index>}, "block_hash": <block_hash>}.

#### Examples

```bash
echo '{
    "id": 2,
    "jsonrpc": "2.0",
    "method": "get_live_cell",
    "params": [
        {
            "block_hash": null,
            "cell": {
                "index": "0",
                "tx_hash": "0xf9cab0387e48d97b6905b54a8f647a5803ced1c4f187360fba0ddad292e618e7"
            }
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
        "cell": {
            "capacity": "34400000000",
            "data_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
            "lock": {
                "args": [],
                "code_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
                "hash_type": "Data"
            },
            "type": null
        },
        "status": "live"
    }
}
```

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
    "result": "1024"
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
        "dao": "0x010000000000000063ae2493990ab00000f0c890408900000061eb7ada030000",
        "difficulty": "0x3e8",
        "epoch": "0",
        "hash": "0x0a663a245ae0b3545a24e4ec06066b9e9f0bd32e5fdbb9038ec15af59ef2593d",
        "number": "1024",
        "parent_hash": "0x5eae6dfdac4b3099619a131975fd0be6da3c099eeaae8261182627a53548fc87",
        "proposals_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
        "seal": {
            "nonce": "0",
            "proof": "0x"
        },
        "timestamp": "1557311767",
        "transactions_root": "0x64a872caca3c7b671b64897230e88dec8800b594347d192661908a87f7fbcb9b",
        "uncles_count": "0",
        "uncles_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
        "version": "0",
        "witnesses_root": "0x688c1ff7df5b9b454bd76a031d40e13b4e462aeee5f8497d0f9f6f1dbd5d8111"
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
        "0xab1c5bbba99b2f775efef276147116c2eb86278c021379cc8c8f3ed12420237f"
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
            "deps": [
                {
                    "block_hash": null,
                    "cell": {
                        "index": "0",
                        "tx_hash": "0xf9cab0387e48d97b6905b54a8f647a5803ced1c4f187360fba0ddad292e618e7"
                    }
                }
            ],
            "hash": "0xab1c5bbba99b2f775efef276147116c2eb86278c021379cc8c8f3ed12420237f",
            "inputs": [
                {
                    "previous_output": {
                        "block_hash": null,
                        "cell": {
                            "index": "0",
                            "tx_hash": "0xb0a82422071490147538402b4cdb1ffad0d8810c24b5745aa7ff3580fc48bf8d"
                        }
                    },
                    "since": "0"
                }
            ],
            "outputs": [
                {
                    "capacity": "100000000000",
                    "data_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
                    "lock": {
                        "args": [],
                        "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
                        "hash_type": "Data"
                    },
                    "type": null
                }
            ],
            "outputs_data": [
                "0x"
            ],
            "version": "0",
            "witnesses": []
        },
        "tx_status": {
            "block_hash": null,
            "status": "pending"
        }
    }
}
```

## Experiment

### `_compute_script_hash`

Returns script hash of given transaction script

**Deprecated**: will be removed in a later version

#### Parameters

    args - Hex encoded arguments passed to reference cell
    code_hash - Code hash of referenced cell
    hash_type - Data: code_hash matches against dep cell data hash; Type: code_hash matches against dep cell type hash.

#### Examples

```bash
echo '{
    "id": 2,
    "jsonrpc": "2.0",
    "method": "_compute_script_hash",
    "params": [
        {
            "args": [],
            "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
            "hash_type": "Data"
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
    "result": "0xa6ed2edad0d48a3d58d0bec407ddf2e40ddd5f533d7059a160149f4021c2a589"
}
```

### `_compute_transaction_hash`

Return the transaction hash

**Deprecated**: will be removed in a later version

#### Parameters

    transaction - The transaction object
    version - Transaction version
    deps - Dependent cells
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
            "deps": [
                {
                    "block_hash": null,
                    "cell": {
                        "index": "0",
                        "tx_hash": "0xf9cab0387e48d97b6905b54a8f647a5803ced1c4f187360fba0ddad292e618e7"
                    }
                }
            ],
            "inputs": [
                {
                    "previous_output": {
                        "block_hash": null,
                        "cell": {
                            "index": "0",
                            "tx_hash": "0xb0a82422071490147538402b4cdb1ffad0d8810c24b5745aa7ff3580fc48bf8d"
                        }
                    },
                    "since": "0"
                }
            ],
            "outputs": [
                {
                    "capacity": "100000000000",
                    "data_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
                    "lock": {
                        "args": [],
                        "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
                        "hash_type": "Data"
                    },
                    "type": null
                }
            ],
            "outputs_data": [
                "0x"
            ],
            "version": "0",
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
    "result": "0xab1c5bbba99b2f775efef276147116c2eb86278c021379cc8c8f3ed12420237f"
}
```

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
            "deps": [
                {
                    "block_hash": null,
                    "cell": {
                        "index": "0",
                        "tx_hash": "0xf9cab0387e48d97b6905b54a8f647a5803ced1c4f187360fba0ddad292e618e7"
                    }
                }
            ],
            "inputs": [
                {
                    "previous_output": {
                        "block_hash": null,
                        "cell": {
                            "index": "0",
                            "tx_hash": "0xb0a82422071490147538402b4cdb1ffad0d8810c24b5745aa7ff3580fc48bf8d"
                        }
                    },
                    "since": "0"
                }
            ],
            "outputs": [
                {
                    "capacity": "100000000000",
                    "data_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
                    "lock": {
                        "args": [],
                        "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
                        "hash_type": "Data"
                    },
                    "type": null
                }
            ],
            "outputs_data": [
                "0x"
            ],
            "version": "0",
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
        "cycles": "12"
    }
}
```

## Indexer

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
        "0xa6ed2edad0d48a3d58d0bec407ddf2e40ddd5f533d7059a160149f4021c2a589"
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

### `get_live_cells_by_lock_hash`

Returns the live cells collection by the hash of lock script.

#### Parameters

    lock_hash - Cell lock script hash
    page - Page number
    per - Page size, max value is 50
    reverse_order - Returns the live cells collection in reverse order, an optional parameter, default is false

#### Examples

```bash
echo '{
    "id": 2,
    "jsonrpc": "2.0",
    "method": "get_live_cells_by_lock_hash",
    "params": [
        "0xa6ed2edad0d48a3d58d0bec407ddf2e40ddd5f533d7059a160149f4021c2a589",
        "0",
        "2"
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
                "capacity": "100000000000",
                "data_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
                "lock": {
                    "args": [],
                    "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
                    "hash_type": "Data"
                },
                "type": null
            },
            "created_by": {
                "block_number": "1",
                "index": "0",
                "tx_hash": "0xb0a82422071490147538402b4cdb1ffad0d8810c24b5745aa7ff3580fc48bf8d"
            }
        },
        {
            "cell_output": {
                "capacity": "100000000000",
                "data_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
                "lock": {
                    "args": [],
                    "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
                    "hash_type": "Data"
                },
                "type": null
            },
            "created_by": {
                "block_number": "2",
                "index": "0",
                "tx_hash": "0x641634c12815acea8518f3a2cb8e8385c5e14e126fa24c40d9222ffb760660ec"
            }
        }
    ]
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
            "block_hash": "0x0a663a245ae0b3545a24e4ec06066b9e9f0bd32e5fdbb9038ec15af59ef2593d",
            "block_number": "1024",
            "lock_hash": "0xa6ed2edad0d48a3d58d0bec407ddf2e40ddd5f533d7059a160149f4021c2a589"
        }
    ]
}
```

### `get_transactions_by_lock_hash`

Returns the transactions collection by the hash of lock script. Returns empty array when the `lock_hash` has not been indexed yet.

#### Parameters

    lock_hash - Cell lock script hash
    page - Page number
    per - Page size, max value is 50
    reverse_order - Return the transactions collection in reverse order, an optional parameter, default is false

#### Examples

```bash
echo '{
    "id": 2,
    "jsonrpc": "2.0",
    "method": "get_transactions_by_lock_hash",
    "params": [
        "0xa6ed2edad0d48a3d58d0bec407ddf2e40ddd5f533d7059a160149f4021c2a589",
        "0",
        "2"
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
                "block_number": "1",
                "index": "0",
                "tx_hash": "0xb0a82422071490147538402b4cdb1ffad0d8810c24b5745aa7ff3580fc48bf8d"
            }
        },
        {
            "consumed_by": null,
            "created_by": {
                "block_number": "2",
                "index": "0",
                "tx_hash": "0x641634c12815acea8518f3a2cb8e8385c5e14e126fa24c40d9222ffb760660ec"
            }
        }
    ]
}
```

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
        "0xa6ed2edad0d48a3d58d0bec407ddf2e40ddd5f533d7059a160149f4021c2a589",
        "1024"
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
        "block_hash": "0x0a663a245ae0b3545a24e4ec06066b9e9f0bd32e5fdbb9038ec15af59ef2593d",
        "block_number": "1024",
        "lock_hash": "0xa6ed2edad0d48a3d58d0bec407ddf2e40ddd5f533d7059a160149f4021c2a589"
    }
}
```

## Net

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
            "ban_until": "1840546800000",
            "created_at": "1562803123000"
        }
    ]
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
                    "score": "1"
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
                    "score": "255"
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
                "score": "255"
            },
            {
                "address": "/ip4/0.0.0.0/tcp/8112/p2p/QmTRHCdrRtgUzYLNCin69zEvPvLYdxUZLLfLYyHVY3DZAS",
                "score": "1"
            }
        ],
        "is_outbound": null,
        "node_id": "QmTRHCdrRtgUzYLNCin69zEvPvLYdxUZLLfLYyHVY3DZAS",
        "version": "0.0.0"
    }
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
        "1840546800000",
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
    deps - Dependent cells
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
            "deps": [
                {
                    "block_hash": null,
                    "cell": {
                        "index": "0",
                        "tx_hash": "0xf9cab0387e48d97b6905b54a8f647a5803ced1c4f187360fba0ddad292e618e7"
                    }
                }
            ],
            "inputs": [
                {
                    "previous_output": {
                        "block_hash": null,
                        "cell": {
                            "index": "0",
                            "tx_hash": "0xb0a82422071490147538402b4cdb1ffad0d8810c24b5745aa7ff3580fc48bf8d"
                        }
                    },
                    "since": "0"
                }
            ],
            "outputs": [
                {
                    "capacity": "100000000000",
                    "data_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
                    "lock": {
                        "args": [],
                        "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
                        "hash_type": "Data"
                    },
                    "type": null
                }
            ],
            "outputs_data": [
                "0x"
            ],
            "version": "0",
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
    "result": "0xab1c5bbba99b2f775efef276147116c2eb86278c021379cc8c8f3ed12420237f"
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
        "last_txs_updated_at": "0",
        "orphan": "0",
        "pending": "1",
        "proposed": "0",
        "total_tx_cycles": "12",
        "total_tx_size": "181"
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
                "id": "42",
                "message": "An example alert message!",
                "notice_until": "2524579200000",
                "priority": "1"
            }
        ],
        "chain": "main",
        "difficulty": "0x3e8",
        "epoch": "0",
        "is_initial_block_download": true,
        "median_time": "1557311762"
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
            "blocks_in_flight": "86",
            "last_updated": "1557289448237",
            "peer": "1"
        }
    ]
}
```

