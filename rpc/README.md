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
        "0xfbabfb104e41150467aa8cc8fc7b60fcf4536f57c0b0609e07bf5250824ad1da"
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
            "dao": "0x0100000000000000005827f2ba13b000d77fa3d595aa00000061eb7ada030000",
            "difficulty": "0x7a1200",
            "epoch": "1",
            "hash": "0xfbabfb104e41150467aa8cc8fc7b60fcf4536f57c0b0609e07bf5250824ad1da",
            "number": "1024",
            "parent_hash": "0xa27dcb73a44f0602cd511cfdffb0acb2731249465f8ac3ab5eff9537f4f45ac6",
            "proposals_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
            "seal": {
                "nonce": "0",
                "proof": "0x"
            },
            "timestamp": "1557311767",
            "transactions_root": "0xf2d214d2211ae5e526b9d4580afc9288d5026312191cb83df2baa1f0e1a421bb",
            "uncles_count": "0",
            "uncles_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
            "version": "0",
            "witnesses_root": "0xa486c1f875971bb1adcdba5fd92a0bb334c052c6a8b97211654a6f00d01730eb"
        },
        "proposals": [],
        "transactions": [
            {
                "cell_deps": [],
                "hash": "0xf2d214d2211ae5e526b9d4580afc9288d5026312191cb83df2baa1f0e1a421bb",
                "header_deps": [],
                "inputs": [
                    {
                        "previous_output": {
                            "index": "4294967295",
                            "tx_hash": "0x0000000000000000000000000000000000000000000000000000000000000000"
                        },
                        "since": "1024"
                    }
                ],
                "outputs": [
                    {
                        "capacity": "70193197506",
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
            "dao": "0x0100000000000000005827f2ba13b000d77fa3d595aa00000061eb7ada030000",
            "difficulty": "0x7a1200",
            "epoch": "1",
            "hash": "0xfbabfb104e41150467aa8cc8fc7b60fcf4536f57c0b0609e07bf5250824ad1da",
            "number": "1024",
            "parent_hash": "0xa27dcb73a44f0602cd511cfdffb0acb2731249465f8ac3ab5eff9537f4f45ac6",
            "proposals_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
            "seal": {
                "nonce": "0",
                "proof": "0x"
            },
            "timestamp": "1557311767",
            "transactions_root": "0xf2d214d2211ae5e526b9d4580afc9288d5026312191cb83df2baa1f0e1a421bb",
            "uncles_count": "0",
            "uncles_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
            "version": "0",
            "witnesses_root": "0xa486c1f875971bb1adcdba5fd92a0bb334c052c6a8b97211654a6f00d01730eb"
        },
        "proposals": [],
        "transactions": [
            {
                "cell_deps": [],
                "hash": "0xf2d214d2211ae5e526b9d4580afc9288d5026312191cb83df2baa1f0e1a421bb",
                "header_deps": [],
                "inputs": [
                    {
                        "previous_output": {
                            "index": "4294967295",
                            "tx_hash": "0x0000000000000000000000000000000000000000000000000000000000000000"
                        },
                        "since": "1024"
                    }
                ],
                "outputs": [
                    {
                        "capacity": "70193197506",
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
    "result": "0xfbabfb104e41150467aa8cc8fc7b60fcf4536f57c0b0609e07bf5250824ad1da"
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
        "0xfbabfb104e41150467aa8cc8fc7b60fcf4536f57c0b0609e07bf5250824ad1da"
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
        "primary": "69444444445",
        "proposal_reward": "0",
        "secondary": "748753061",
        "total": "70193197506",
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
            "block_hash": "0x999fc06c5024a3d73309737d501e13e345c6a646b05109610359dca22007a780",
            "capacity": "125000000000",
            "lock": {
                "args": [],
                "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
                "hash_type": "Data"
            },
            "out_point": {
                "index": "0",
                "tx_hash": "0x8128f99d3ea2d2683dd3ed4e8dc595b3ab58862e7b35c724ca063834e0fe2a80"
            }
        },
        {
            "block_hash": "0x5c6b5cee7547076d773d23efbd3ef9f6c823dfc9ec97ebbf5014fc5670a3f692",
            "capacity": "125000000000",
            "lock": {
                "args": [],
                "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
                "hash_type": "Data"
            },
            "out_point": {
                "index": "0",
                "tx_hash": "0x2df11ec74867e18c74d0f8748fd07e0381f8ab8333de6eff431a6d2c949c1290"
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
        "0xfbabfb104e41150467aa8cc8fc7b60fcf4536f57c0b0609e07bf5250824ad1da"
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
        "dao": "0x0100000000000000005827f2ba13b000d77fa3d595aa00000061eb7ada030000",
        "difficulty": "0x7a1200",
        "epoch": "1",
        "hash": "0xfbabfb104e41150467aa8cc8fc7b60fcf4536f57c0b0609e07bf5250824ad1da",
        "number": "1024",
        "parent_hash": "0xa27dcb73a44f0602cd511cfdffb0acb2731249465f8ac3ab5eff9537f4f45ac6",
        "proposals_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
        "seal": {
            "nonce": "0",
            "proof": "0x"
        },
        "timestamp": "1557311767",
        "transactions_root": "0xf2d214d2211ae5e526b9d4580afc9288d5026312191cb83df2baa1f0e1a421bb",
        "uncles_count": "0",
        "uncles_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
        "version": "0",
        "witnesses_root": "0xa486c1f875971bb1adcdba5fd92a0bb334c052c6a8b97211654a6f00d01730eb"
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
        "dao": "0x0100000000000000005827f2ba13b000d77fa3d595aa00000061eb7ada030000",
        "difficulty": "0x7a1200",
        "epoch": "1",
        "hash": "0xfbabfb104e41150467aa8cc8fc7b60fcf4536f57c0b0609e07bf5250824ad1da",
        "number": "1024",
        "parent_hash": "0xa27dcb73a44f0602cd511cfdffb0acb2731249465f8ac3ab5eff9537f4f45ac6",
        "proposals_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
        "seal": {
            "nonce": "0",
            "proof": "0x"
        },
        "timestamp": "1557311767",
        "transactions_root": "0xf2d214d2211ae5e526b9d4580afc9288d5026312191cb83df2baa1f0e1a421bb",
        "uncles_count": "0",
        "uncles_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
        "version": "0",
        "witnesses_root": "0xa486c1f875971bb1adcdba5fd92a0bb334c052c6a8b97211654a6f00d01730eb"
    }
}
```

### `get_live_cell`

Returns the information about a cell by out_point if it is live.

#### Parameters

    out_point - OutPoint object {{"tx_hash": <tx_hash>, "index": <index>}}.

#### Examples

```bash
echo '{
    "id": 2,
    "jsonrpc": "2.0",
    "method": "get_live_cell",
    "params": [
        {
            "index": "0",
            "tx_hash": "0xb62c98b6ed8fda85d56362e3c27586f1ea309ca5949506b3ed7b9ff094828df0"
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
        "dao": "0x0100000000000000005827f2ba13b000d77fa3d595aa00000061eb7ada030000",
        "difficulty": "0x7a1200",
        "epoch": "1",
        "hash": "0xfbabfb104e41150467aa8cc8fc7b60fcf4536f57c0b0609e07bf5250824ad1da",
        "number": "1024",
        "parent_hash": "0xa27dcb73a44f0602cd511cfdffb0acb2731249465f8ac3ab5eff9537f4f45ac6",
        "proposals_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
        "seal": {
            "nonce": "0",
            "proof": "0x"
        },
        "timestamp": "1557311767",
        "transactions_root": "0xf2d214d2211ae5e526b9d4580afc9288d5026312191cb83df2baa1f0e1a421bb",
        "uncles_count": "0",
        "uncles_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
        "version": "0",
        "witnesses_root": "0xa486c1f875971bb1adcdba5fd92a0bb334c052c6a8b97211654a6f00d01730eb"
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
        "0x3a6db7279c45e5b24ae7fb7624aa4d599eee2c4224cf379c23e6c1d4464ae60f"
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
                    "is_dep_group": false,
                    "out_point": {
                        "index": "0",
                        "tx_hash": "0xb62c98b6ed8fda85d56362e3c27586f1ea309ca5949506b3ed7b9ff094828df0"
                    }
                }
            ],
            "hash": "0x3a6db7279c45e5b24ae7fb7624aa4d599eee2c4224cf379c23e6c1d4464ae60f",
            "header_deps": [
                "0x968e857abd52bcf416d4b4065d4334ec4c6a0e02a04812358540b8bfcfabcf47"
            ],
            "inputs": [
                {
                    "previous_output": {
                        "index": "0",
                        "tx_hash": "0x8128f99d3ea2d2683dd3ed4e8dc595b3ab58862e7b35c724ca063834e0fe2a80"
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
                    "is_dep_group": false,
                    "out_point": {
                        "index": "0",
                        "tx_hash": "0xb62c98b6ed8fda85d56362e3c27586f1ea309ca5949506b3ed7b9ff094828df0"
                    }
                }
            ],
            "header_deps": [
                "0x968e857abd52bcf416d4b4065d4334ec4c6a0e02a04812358540b8bfcfabcf47"
            ],
            "inputs": [
                {
                    "previous_output": {
                        "index": "0",
                        "tx_hash": "0x8128f99d3ea2d2683dd3ed4e8dc595b3ab58862e7b35c724ca063834e0fe2a80"
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
    "result": "0x3a6db7279c45e5b24ae7fb7624aa4d599eee2c4224cf379c23e6c1d4464ae60f"
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
            "cell_deps": [
                {
                    "is_dep_group": false,
                    "out_point": {
                        "index": "0",
                        "tx_hash": "0xb62c98b6ed8fda85d56362e3c27586f1ea309ca5949506b3ed7b9ff094828df0"
                    }
                }
            ],
            "header_deps": [
                "0x968e857abd52bcf416d4b4065d4334ec4c6a0e02a04812358540b8bfcfabcf47"
            ],
            "inputs": [
                {
                    "previous_output": {
                        "index": "0",
                        "tx_hash": "0x8128f99d3ea2d2683dd3ed4e8dc595b3ab58862e7b35c724ca063834e0fe2a80"
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
                "capacity": "125000000000",
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
                "tx_hash": "0x8128f99d3ea2d2683dd3ed4e8dc595b3ab58862e7b35c724ca063834e0fe2a80"
            }
        },
        {
            "cell_output": {
                "capacity": "125000000000",
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
                "tx_hash": "0x2df11ec74867e18c74d0f8748fd07e0381f8ab8333de6eff431a6d2c949c1290"
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
            "block_hash": "0xfbabfb104e41150467aa8cc8fc7b60fcf4536f57c0b0609e07bf5250824ad1da",
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
                "tx_hash": "0x8128f99d3ea2d2683dd3ed4e8dc595b3ab58862e7b35c724ca063834e0fe2a80"
            }
        },
        {
            "consumed_by": null,
            "created_by": {
                "block_number": "2",
                "index": "0",
                "tx_hash": "0x2df11ec74867e18c74d0f8748fd07e0381f8ab8333de6eff431a6d2c949c1290"
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
        "block_hash": "0xfbabfb104e41150467aa8cc8fc7b60fcf4536f57c0b0609e07bf5250824ad1da",
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
                    "is_dep_group": false,
                    "out_point": {
                        "index": "0",
                        "tx_hash": "0xb62c98b6ed8fda85d56362e3c27586f1ea309ca5949506b3ed7b9ff094828df0"
                    }
                }
            ],
            "header_deps": [
                "0x968e857abd52bcf416d4b4065d4334ec4c6a0e02a04812358540b8bfcfabcf47"
            ],
            "inputs": [
                {
                    "previous_output": {
                        "index": "0",
                        "tx_hash": "0x8128f99d3ea2d2683dd3ed4e8dc595b3ab58862e7b35c724ca063834e0fe2a80"
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
    "result": "0x3a6db7279c45e5b24ae7fb7624aa4d599eee2c4224cf379c23e6c1d4464ae60f"
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
        "total_tx_size": "218"
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
        "difficulty": "0x7a1200",
        "epoch": "1",
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
