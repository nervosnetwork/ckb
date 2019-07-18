# CKB JSON-RPC Protocols


*   [`Chain`](#chain)
    *   [`get_block`](#get_block)
    *   [`get_block_by_number`](#get_block_by_number)
    *   [`get_block_hash`](#get_block_hash)
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
    *   [`_compute_code_hash`](#_compute_code_hash)
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
        "0x2d6d06b953787bc8b4ff3d4d812413487022860f09c5026f9ca0c130c583a344"
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
            "dao": "0x01000000000000000000c16ff286230000203d88792d0000000961f400000000",
            "difficulty": "0x3e8",
            "epoch": "0",
            "hash": "0x2d6d06b953787bc8b4ff3d4d812413487022860f09c5026f9ca0c130c583a344",
            "number": "2",
            "parent_hash": "0x2de0ba77b1a3c97be964c43a00d19963d47723af34bcae759dbe44625bb848c5",
            "proposals_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
            "seal": {
                "nonce": "0",
                "proof": "0x"
            },
            "timestamp": "1557310745",
            "transactions_root": "0x81766f545198f7cb095d20bdf1ceb1fa7a3d9caf9dae0409370f334233e9c816",
            "uncles_count": "0",
            "uncles_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
            "version": "0",
            "witnesses_root": "0x7ef131cca955519c025ddd4d3a3a92f52ffef67a967d3d867f5f6d1147fe2937"
        },
        "proposals": [],
        "transactions": [
            {
                "deps": [],
                "hash": "0x81766f545198f7cb095d20bdf1ceb1fa7a3d9caf9dae0409370f334233e9c816",
                "inputs": [
                    {
                        "previous_output": {
                            "block_hash": null,
                            "cell": null
                        },
                        "since": "2"
                    }
                ],
                "outputs": [
                    {
                        "capacity": "50000000000000",
                        "data": "0x",
                        "lock": {
                            "args": [],
                            "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
                            "hash_type": "Data"
                        },
                        "type": null
                    }
                ],
                "version": "0",
                "witnesses": []
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
            "dao": "0x01000000000000000000c16ff286230000203d88792d0000000961f400000000",
            "difficulty": "0x3e8",
            "epoch": "0",
            "hash": "0x272cc0ca35fb927e1f7cdb096355b6c1eefd29f475161e6670a5c5dce58c418d",
            "number": "1024",
            "parent_hash": "0xc66ee8f62599c7e17ef6d8f42411fdf08def85ca12d603a0f80052eab6c5abf9",
            "proposals_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
            "seal": {
                "nonce": "0",
                "proof": "0x"
            },
            "timestamp": "1557311767",
            "transactions_root": "0x81389ffabda2d1658c75a81e390a82b335d4cb849b2269d134b042fea9cb9513",
            "uncles_count": "0",
            "uncles_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
            "version": "0",
            "witnesses_root": "0x106005deb92716b3f4497c9971c2c031ff6bfc7c5731c8821926cc0b0cba50cc"
        },
        "proposals": [],
        "transactions": [
            {
                "deps": [],
                "hash": "0x81389ffabda2d1658c75a81e390a82b335d4cb849b2269d134b042fea9cb9513",
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
                        "capacity": "50000000000000",
                        "data": "0x",
                        "lock": {
                            "args": [],
                            "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
                            "hash_type": "Data"
                        },
                        "type": null
                    }
                ],
                "version": "0",
                "witnesses": []
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
    "result": "0x2d6d06b953787bc8b4ff3d4d812413487022860f09c5026f9ca0c130c583a344"
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
        "2",
        "5"
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
            "capacity": "50000000000000",
            "lock": {
                "args": [],
                "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
                "hash_type": "Data"
            },
            "out_point": {
                "block_hash": "0x2d6d06b953787bc8b4ff3d4d812413487022860f09c5026f9ca0c130c583a344",
                "cell": {
                    "index": "0",
                    "tx_hash": "0x81766f545198f7cb095d20bdf1ceb1fa7a3d9caf9dae0409370f334233e9c816"
                }
            }
        },
        {
            "capacity": "50000000000000",
            "lock": {
                "args": [],
                "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
                "hash_type": "Data"
            },
            "out_point": {
                "block_hash": "0xf458a2800aa1da2b05656f966bb8d3a2326a04c38f355beebb48bde31c053ab7",
                "cell": {
                    "index": "0",
                    "tx_hash": "0xe84a3383d724d5a82db3335870e424d3a706ac64d8ceeb4716387ec37c346659"
                }
            }
        },
        {
            "capacity": "50000000000000",
            "lock": {
                "args": [],
                "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
                "hash_type": "Data"
            },
            "out_point": {
                "block_hash": "0xa191d9800b19475b2541feef25559fe137f4cd86ad52d0eaddc0c4e0ec1257de",
                "cell": {
                    "index": "0",
                    "tx_hash": "0x1c5f992b8a7c5e13b5ab6aedadb4839cf135627f100cdf44a2ec333a60b44e8c"
                }
            }
        },
        {
            "capacity": "50000000000000",
            "lock": {
                "args": [],
                "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
                "hash_type": "Data"
            },
            "out_point": {
                "block_hash": "0xd010752a6a30ecb8303b844320d9f577d840f2a050c010e8f33ba46f3a1d13c7",
                "cell": {
                    "index": "0",
                    "tx_hash": "0x01f306cfbd5985f7275d3a91230b84773e698bff195ade13787c2316bd4c8b19"
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
        "difficulty": "0x3e8",
        "epoch_reward": "125000000000000",
        "length": "1250",
        "number": "0",
        "start_number": "0"
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
        "epoch_reward": "125000000000000",
        "length": "1250",
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
        "0x272cc0ca35fb927e1f7cdb096355b6c1eefd29f475161e6670a5c5dce58c418d"
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
        "dao": "0x01000000000000000000c16ff286230000203d88792d0000000961f400000000",
        "difficulty": "0x3e8",
        "epoch": "0",
        "hash": "0x272cc0ca35fb927e1f7cdb096355b6c1eefd29f475161e6670a5c5dce58c418d",
        "number": "1024",
        "parent_hash": "0xc66ee8f62599c7e17ef6d8f42411fdf08def85ca12d603a0f80052eab6c5abf9",
        "proposals_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
        "seal": {
            "nonce": "0",
            "proof": "0x"
        },
        "timestamp": "1557311767",
        "transactions_root": "0x81389ffabda2d1658c75a81e390a82b335d4cb849b2269d134b042fea9cb9513",
        "uncles_count": "0",
        "uncles_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
        "version": "0",
        "witnesses_root": "0x106005deb92716b3f4497c9971c2c031ff6bfc7c5731c8821926cc0b0cba50cc"
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
        "dao": "0x01000000000000000000c16ff286230000203d88792d0000000961f400000000",
        "difficulty": "0x3e8",
        "epoch": "0",
        "hash": "0x272cc0ca35fb927e1f7cdb096355b6c1eefd29f475161e6670a5c5dce58c418d",
        "number": "1024",
        "parent_hash": "0xc66ee8f62599c7e17ef6d8f42411fdf08def85ca12d603a0f80052eab6c5abf9",
        "proposals_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
        "seal": {
            "nonce": "0",
            "proof": "0x"
        },
        "timestamp": "1557311767",
        "transactions_root": "0x81389ffabda2d1658c75a81e390a82b335d4cb849b2269d134b042fea9cb9513",
        "uncles_count": "0",
        "uncles_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
        "version": "0",
        "witnesses_root": "0x106005deb92716b3f4497c9971c2c031ff6bfc7c5731c8821926cc0b0cba50cc"
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
                "tx_hash": "0x01f306cfbd5985f7275d3a91230b84773e698bff195ade13787c2316bd4c8b19"
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
            "capacity": "50000000000000",
            "data": "0x",
            "lock": {
                "args": [],
                "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
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
        "dao": "0x01000000000000000000c16ff286230000203d88792d0000000961f400000000",
        "difficulty": "0x3e8",
        "epoch": "0",
        "hash": "0x272cc0ca35fb927e1f7cdb096355b6c1eefd29f475161e6670a5c5dce58c418d",
        "number": "1024",
        "parent_hash": "0xc66ee8f62599c7e17ef6d8f42411fdf08def85ca12d603a0f80052eab6c5abf9",
        "proposals_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
        "seal": {
            "nonce": "0",
            "proof": "0x"
        },
        "timestamp": "1557311767",
        "transactions_root": "0x81389ffabda2d1658c75a81e390a82b335d4cb849b2269d134b042fea9cb9513",
        "uncles_count": "0",
        "uncles_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
        "version": "0",
        "witnesses_root": "0x106005deb92716b3f4497c9971c2c031ff6bfc7c5731c8821926cc0b0cba50cc"
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
        "0x81389ffabda2d1658c75a81e390a82b335d4cb849b2269d134b042fea9cb9513"
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
            "deps": [],
            "hash": "0x81389ffabda2d1658c75a81e390a82b335d4cb849b2269d134b042fea9cb9513",
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
                    "capacity": "50000000000000",
                    "data": "0x",
                    "lock": {
                        "args": [],
                        "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
                        "hash_type": "Data"
                    },
                    "type": null
                }
            ],
            "version": "0",
            "witnesses": []
        },
        "tx_status": {
            "block_hash": "0x272cc0ca35fb927e1f7cdb096355b6c1eefd29f475161e6670a5c5dce58c418d",
            "status": "committed"
        }
    }
}
```

## Experiment

### `_compute_code_hash`

Returns code hash of given hex encoded data

**Deprecated**: will be removed in a later version

#### Parameters

    data - Hex encoded data

#### Examples

```bash
echo '{
    "id": 2,
    "jsonrpc": "2.0",
    "method": "_compute_code_hash",
    "params": [
        "0x123456"
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
    "result": "0x7dacea2e6ae8131b7f187570135ebb1b217a69458b3eae350104942c06939783"
}
```

### `_compute_script_hash`

Returns script hash of given transaction script

**Deprecated**: will be removed in a later version

#### Parameters

    args - Hex encoded arguments passed to reference cell
    code_hash - Code hash of referenced cell

#### Examples

```bash
echo '{
    "id": 2,
    "jsonrpc": "2.0",
    "method": "_compute_script_hash",
    "params": [
        {
            "args": [
                "0x123450",
                "0x678900"
            ],
            "code_hash": "0xb35557e7e9854206f7bc13e3c3a7fa4cf8892c84a09237fb0aab40aab3771eee",
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
    "result": "0xa5b2c97e57901ca70bfa30a18f8bb4dac4f666ccbcdccd536b33a8066d01dd8e"
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
            "deps": [],
            "inputs": [
                {
                    "args": [],
                    "previous_output": {
                        "block_hash": "0x272cc0ca35fb927e1f7cdb096355b6c1eefd29f475161e6670a5c5dce58c418d",
                        "cell": {
                            "index": "0",
                            "tx_hash": "0x81389ffabda2d1658c75a81e390a82b335d4cb849b2269d134b042fea9cb9513"
                        }
                    },
                    "since": "0"
                }
            ],
            "outputs": [
                {
                    "capacity": "100000000000",
                    "data": "0x",
                    "lock": {
                        "args": [],
                        "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
                        "hash_type": "Data"
                    },
                    "type": null
                }
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
    "result": "0x10bffb7b67bfba00efcce595a07f6a5a648b97b299a5be632af15b6d06398246"
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
                    "cell": {
                        "index": "0",
                        "tx_hash": "0x03c957dcb46e71542d47a8d5d86dc1e27a48e885ee07bb399fca5d2d8d37f626"
                    }
                }
            ],
            "inputs": [
                {
                    "args": [],
                    "previous_output": {
                        "block_hash": "0x272cc0ca35fb927e1f7cdb096355b6c1eefd29f475161e6670a5c5dce58c418d",
                        "cell": {
                            "index": "0",
                            "tx_hash": "0x81389ffabda2d1658c75a81e390a82b335d4cb849b2269d134b042fea9cb9513"
                        }
                    },
                    "since": "0"
                }
            ],
            "outputs": [
                {
                    "capacity": "100000000000",
                    "data": "0x",
                    "lock": {
                        "args": [],
                        "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
                        "hash_type": "Data"
                    },
                    "type": null
                }
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
                "capacity": "50000000000000",
                "data": "0x",
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
                "tx_hash": "0xa3506ca3babb482061f60ae06a069d2d9bd4a42ea905db42456310ea74006e40"
            }
        },
        {
            "cell_output": {
                "capacity": "50000000000000",
                "data": "0x",
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
                "tx_hash": "0x81766f545198f7cb095d20bdf1ceb1fa7a3d9caf9dae0409370f334233e9c816"
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
            "block_hash": "0x272cc0ca35fb927e1f7cdb096355b6c1eefd29f475161e6670a5c5dce58c418d",
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
                "tx_hash": "0xa3506ca3babb482061f60ae06a069d2d9bd4a42ea905db42456310ea74006e40"
            }
        },
        {
            "consumed_by": null,
            "created_by": {
                "block_number": "2",
                "index": "0",
                "tx_hash": "0x81766f545198f7cb095d20bdf1ceb1fa7a3d9caf9dae0409370f334233e9c816"
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
        "block_hash": "0x272cc0ca35fb927e1f7cdb096355b6c1eefd29f475161e6670a5c5dce58c418d",
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
        "version": "0.9.0"
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
        "test set_ban rpc"
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
                    "cell": {
                        "index": "0",
                        "tx_hash": "0x03c957dcb46e71542d47a8d5d86dc1e27a48e885ee07bb399fca5d2d8d37f626"
                    }
                }
            ],
            "inputs": [
                {
                    "args": [],
                    "previous_output": {
                        "block_hash": "0x272cc0ca35fb927e1f7cdb096355b6c1eefd29f475161e6670a5c5dce58c418d",
                        "cell": {
                            "index": "0",
                            "tx_hash": "0x81389ffabda2d1658c75a81e390a82b335d4cb849b2269d134b042fea9cb9513"
                        }
                    },
                    "since": "0"
                }
            ],
            "outputs": [
                {
                    "capacity": "100000000000",
                    "data": "0x",
                    "lock": {
                        "args": [],
                        "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
                        "hash_type": "Data"
                    },
                    "type": null
                }
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
    "result": "0xc00763e95b225031df58c204d65089414482e629fd2f52b19e2c7e21eaa179ba"
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

