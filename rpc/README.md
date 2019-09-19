# CKB JSON-RPC Protocols


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
    *   [`_compute_script_hash`](#_compute_script_hash)
*   [`Indexer`](#indexer)
    *   [`index_lock_hash`](#index_lock_hash)
    *   [`get_lock_hash_index_states`](#get_lock_hash_index_states)
    *   [`get_live_cells_by_lock_hash`](#get_live_cells_by_lock_hash)
    *   [`get_transactions_by_lock_hash`](#get_transactions_by_lock_hash)
    *   [`deindex_lock_hash`](#deindex_lock_hash)
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
        "dao": "0x0100000000000000005827f2ba13b000d77fa3d595aa00000061eb7ada030000",
        "difficulty": "0x7a1200",
        "epoch": "0x7080018000001",
        "hash": "0xc73a331428dd9ef69b8073c248bfae9dc7c27942bb1cb70581e880bd3020d7da",
        "nonce": "0x0",
        "number": "0x400",
        "parent_hash": "0x956315644ef52193db540709d3a34c7149cfb173e4eedcc64ee10aa366795439",
        "proposals_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
        "timestamp": "0x5cd2b117",
        "transactions_root": "0x8ad0468383d0085e26d9c3b9b648623e4194efc53a03b7cd1a79e92700687f1e",
        "uncles_count": "0x0",
        "uncles_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
        "version": "0x0",
        "witnesses_root": "0x90445a0795a2d7d4af033ec0282a8a1f68f11ffb1cd091b95c2c5515a8336e9c"
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
        "difficulty": "0x7a1200",
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
        "difficulty": "0x3e8",
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
    "result": "0xc73a331428dd9ef69b8073c248bfae9dc7c27942bb1cb70581e880bd3020d7da"
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
        "0xc73a331428dd9ef69b8073c248bfae9dc7c27942bb1cb70581e880bd3020d7da"
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
            "epoch": "0x7080018000001",
            "hash": "0xc73a331428dd9ef69b8073c248bfae9dc7c27942bb1cb70581e880bd3020d7da",
            "nonce": "0x0",
            "number": "0x400",
            "parent_hash": "0x956315644ef52193db540709d3a34c7149cfb173e4eedcc64ee10aa366795439",
            "proposals_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
            "timestamp": "0x5cd2b117",
            "transactions_root": "0x8ad0468383d0085e26d9c3b9b648623e4194efc53a03b7cd1a79e92700687f1e",
            "uncles_count": "0x0",
            "uncles_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
            "version": "0x0",
            "witnesses_root": "0x90445a0795a2d7d4af033ec0282a8a1f68f11ffb1cd091b95c2c5515a8336e9c"
        },
        "proposals": [],
        "transactions": [
            {
                "cell_deps": [],
                "hash": "0x8ad0468383d0085e26d9c3b9b648623e4194efc53a03b7cd1a79e92700687f1e",
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
                        "capacity": "0x1057d731c2",
                        "lock": {
                            "args": [],
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

### `get_header`

Returns the information about a block header by hash.


#### Examples

```bash
echo '{
    "id": 2,
    "jsonrpc": "2.0",
    "method": "get_header",
    "params": [
        "0xc73a331428dd9ef69b8073c248bfae9dc7c27942bb1cb70581e880bd3020d7da"
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
        "epoch": "0x7080018000001",
        "hash": "0xc73a331428dd9ef69b8073c248bfae9dc7c27942bb1cb70581e880bd3020d7da",
        "nonce": "0x0",
        "number": "0x400",
        "parent_hash": "0x956315644ef52193db540709d3a34c7149cfb173e4eedcc64ee10aa366795439",
        "proposals_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
        "timestamp": "0x5cd2b117",
        "transactions_root": "0x8ad0468383d0085e26d9c3b9b648623e4194efc53a03b7cd1a79e92700687f1e",
        "uncles_count": "0x0",
        "uncles_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
        "version": "0x0",
        "witnesses_root": "0x90445a0795a2d7d4af033ec0282a8a1f68f11ffb1cd091b95c2c5515a8336e9c"
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
        "dao": "0x0100000000000000005827f2ba13b000d77fa3d595aa00000061eb7ada030000",
        "difficulty": "0x7a1200",
        "epoch": "0x7080018000001",
        "hash": "0xc73a331428dd9ef69b8073c248bfae9dc7c27942bb1cb70581e880bd3020d7da",
        "nonce": "0x0",
        "number": "0x400",
        "parent_hash": "0x956315644ef52193db540709d3a34c7149cfb173e4eedcc64ee10aa366795439",
        "proposals_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
        "timestamp": "0x5cd2b117",
        "transactions_root": "0x8ad0468383d0085e26d9c3b9b648623e4194efc53a03b7cd1a79e92700687f1e",
        "uncles_count": "0x0",
        "uncles_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
        "version": "0x0",
        "witnesses_root": "0x90445a0795a2d7d4af033ec0282a8a1f68f11ffb1cd091b95c2c5515a8336e9c"
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
        "0xd8753dd87c7dd293d9b64d4ca20d77bb8e5f2d92bf08234b026e2d8b1b00e7e9",
        "0x0",
        "0x2"
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
            "block_hash": "0x03935a4b5e3c03a9c1deb93a39183a9a116c16cff3dc9ab129e847487da0e2b8",
            "capacity": "0x1d1a94a200",
            "lock": {
                "args": [],
                "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
                "hash_type": "data"
            },
            "out_point": {
                "index": "0x0",
                "tx_hash": "0x5ba156200c6310bf140fbbd3bfe7e8f03d4d5f82b612c1a8ec2501826eaabc17"
            }
        },
        {
            "block_hash": "0x813edf971bf6d191335a99c52d90dcea1d2d5195386ce4898d9dfe53e81e4fcb",
            "capacity": "0x1d1a94a200",
            "lock": {
                "args": [],
                "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
                "hash_type": "data"
            },
            "out_point": {
                "index": "0x0",
                "tx_hash": "0x2e32fd60f965075a9a532c670b6d5475a2417e88872b74069e8076e58906b7bf"
            }
        }
    ]
}
```

### `get_live_cell`

Returns the information about a cell by out_point if it is live. If second with_data argument set to true, will return cell data and data_hash if it is live

#### Parameters

    out_point - OutPoint object {"tx_hash": <tx_hash>, "index": <index>}.

#### Examples

```bash
echo '{
    "id": 2,
    "jsonrpc": "2.0",
    "method": "get_live_cell",
    "params": [
        {
            "index": "0x0",
            "tx_hash": "0x29f94532fb6c7a17f13bcde5adb6e2921776ee6f357adf645e5393bd13442141"
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
                    "args": [],
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
        "0xba86cc2cb21832bf4a84c032eb6e8dc422385cc8f8efb84eb0bc5fe0b0b9aece"
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
                        "tx_hash": "0x29f94532fb6c7a17f13bcde5adb6e2921776ee6f357adf645e5393bd13442141"
                    }
                }
            ],
            "hash": "0xba86cc2cb21832bf4a84c032eb6e8dc422385cc8f8efb84eb0bc5fe0b0b9aece",
            "header_deps": [
                "0x8033e126475d197f2366bbc2f30b907d15af85c9d9533253c6f0787dcbbb509e"
            ],
            "inputs": [
                {
                    "previous_output": {
                        "index": "0x0",
                        "tx_hash": "0x5ba156200c6310bf140fbbd3bfe7e8f03d4d5f82b612c1a8ec2501826eaabc17"
                    },
                    "since": "0x0"
                }
            ],
            "outputs": [
                {
                    "capacity": "0x174876e800",
                    "lock": {
                        "args": [],
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
        "0xc73a331428dd9ef69b8073c248bfae9dc7c27942bb1cb70581e880bd3020d7da"
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
        "primary": "0x102b36211d",
        "proposal_reward": "0x0",
        "secondary": "0x2ca110a5",
        "total": "0x1057d731c2",
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
            "dao": "0x0100000000000000005827f2ba13b000d77fa3d595aa00000061eb7ada030000",
            "difficulty": "0x7a1200",
            "epoch": "0x7080018000001",
            "hash": "0xc73a331428dd9ef69b8073c248bfae9dc7c27942bb1cb70581e880bd3020d7da",
            "nonce": "0x0",
            "number": "0x400",
            "parent_hash": "0x956315644ef52193db540709d3a34c7149cfb173e4eedcc64ee10aa366795439",
            "proposals_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
            "timestamp": "0x5cd2b117",
            "transactions_root": "0x8ad0468383d0085e26d9c3b9b648623e4194efc53a03b7cd1a79e92700687f1e",
            "uncles_count": "0x0",
            "uncles_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
            "version": "0x0",
            "witnesses_root": "0x90445a0795a2d7d4af033ec0282a8a1f68f11ffb1cd091b95c2c5515a8336e9c"
        },
        "proposals": [],
        "transactions": [
            {
                "cell_deps": [],
                "hash": "0x8ad0468383d0085e26d9c3b9b648623e4194efc53a03b7cd1a79e92700687f1e",
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
                        "capacity": "0x1057d731c2",
                        "lock": {
                            "args": [],
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
                        "tx_hash": "0x29f94532fb6c7a17f13bcde5adb6e2921776ee6f357adf645e5393bd13442141"
                    }
                }
            ],
            "header_deps": [
                "0x8033e126475d197f2366bbc2f30b907d15af85c9d9533253c6f0787dcbbb509e"
            ],
            "inputs": [
                {
                    "previous_output": {
                        "index": "0x0",
                        "tx_hash": "0x5ba156200c6310bf140fbbd3bfe7e8f03d4d5f82b612c1a8ec2501826eaabc17"
                    },
                    "since": "0x0"
                }
            ],
            "outputs": [
                {
                    "capacity": "0x174876e800",
                    "lock": {
                        "args": [],
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
        "cycles": "0xc"
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
                        "tx_hash": "0x29f94532fb6c7a17f13bcde5adb6e2921776ee6f357adf645e5393bd13442141"
                    }
                }
            ],
            "header_deps": [
                "0x8033e126475d197f2366bbc2f30b907d15af85c9d9533253c6f0787dcbbb509e"
            ],
            "inputs": [
                {
                    "previous_output": {
                        "index": "0x0",
                        "tx_hash": "0x5ba156200c6310bf140fbbd3bfe7e8f03d4d5f82b612c1a8ec2501826eaabc17"
                    },
                    "since": "0x0"
                }
            ],
            "outputs": [
                {
                    "capacity": "0x174876e800",
                    "lock": {
                        "args": [],
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
    "result": "0xba86cc2cb21832bf4a84c032eb6e8dc422385cc8f8efb84eb0bc5fe0b0b9aece"
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
            "tx_hash": "0x29f94532fb6c7a17f13bcde5adb6e2921776ee6f357adf645e5393bd13442141"
        },
        "0xc73a331428dd9ef69b8073c248bfae9dc7c27942bb1cb70581e880bd3020d7da"
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
            "args": [],
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
    "result": "0xd8753dd87c7dd293d9b64d4ca20d77bb8e5f2d92bf08234b026e2d8b1b00e7e9"
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
        "0xd8753dd87c7dd293d9b64d4ca20d77bb8e5f2d92bf08234b026e2d8b1b00e7e9",
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
        "block_hash": "0xc73a331428dd9ef69b8073c248bfae9dc7c27942bb1cb70581e880bd3020d7da",
        "block_number": "0x400",
        "lock_hash": "0xd8753dd87c7dd293d9b64d4ca20d77bb8e5f2d92bf08234b026e2d8b1b00e7e9"
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
            "block_hash": "0xc73a331428dd9ef69b8073c248bfae9dc7c27942bb1cb70581e880bd3020d7da",
            "block_number": "0x400",
            "lock_hash": "0xd8753dd87c7dd293d9b64d4ca20d77bb8e5f2d92bf08234b026e2d8b1b00e7e9"
        }
    ]
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
        "0xd8753dd87c7dd293d9b64d4ca20d77bb8e5f2d92bf08234b026e2d8b1b00e7e9",
        "0x0",
        "0x2"
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
                "capacity": "0x1d1a94a200",
                "lock": {
                    "args": [],
                    "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
                    "hash_type": "data"
                },
                "type": null
            },
            "created_by": {
                "block_number": "0x1",
                "index": "0x0",
                "tx_hash": "0x5ba156200c6310bf140fbbd3bfe7e8f03d4d5f82b612c1a8ec2501826eaabc17"
            }
        },
        {
            "cell_output": {
                "capacity": "0x1d1a94a200",
                "lock": {
                    "args": [],
                    "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
                    "hash_type": "data"
                },
                "type": null
            },
            "created_by": {
                "block_number": "0x2",
                "index": "0x0",
                "tx_hash": "0x2e32fd60f965075a9a532c670b6d5475a2417e88872b74069e8076e58906b7bf"
            }
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
        "0xd8753dd87c7dd293d9b64d4ca20d77bb8e5f2d92bf08234b026e2d8b1b00e7e9",
        "0x0",
        "0x2"
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
                "block_number": "0x1",
                "index": "0x0",
                "tx_hash": "0x5ba156200c6310bf140fbbd3bfe7e8f03d4d5f82b612c1a8ec2501826eaabc17"
            }
        },
        {
            "consumed_by": null,
            "created_by": {
                "block_number": "0x2",
                "index": "0x0",
                "tx_hash": "0x2e32fd60f965075a9a532c670b6d5475a2417e88872b74069e8076e58906b7bf"
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
        "0xd8753dd87c7dd293d9b64d4ca20d77bb8e5f2d92bf08234b026e2d8b1b00e7e9"
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
                        "tx_hash": "0x29f94532fb6c7a17f13bcde5adb6e2921776ee6f357adf645e5393bd13442141"
                    }
                }
            ],
            "header_deps": [
                "0x8033e126475d197f2366bbc2f30b907d15af85c9d9533253c6f0787dcbbb509e"
            ],
            "inputs": [
                {
                    "previous_output": {
                        "index": "0x0",
                        "tx_hash": "0x5ba156200c6310bf140fbbd3bfe7e8f03d4d5f82b612c1a8ec2501826eaabc17"
                    },
                    "since": "0x0"
                }
            ],
            "outputs": [
                {
                    "capacity": "0x174876e800",
                    "lock": {
                        "args": [],
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
    "result": "0xba86cc2cb21832bf4a84c032eb6e8dc422385cc8f8efb84eb0bc5fe0b0b9aece"
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
        "total_tx_cycles": "0xc",
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
        "difficulty": "0x7a1200",
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
