# CKB JSON-RPC Protocols

NOTE: This file is auto-generated. Please don't update this file directly; instead make changes to `rpc/json/rpc.json` and re-run `make gen-rpc-doc`


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
        "dao": "0xb54bdd7f6be90000bb52f392d41cd70024f7ef29b437000000febffacf030000",
        "epoch": "0x7080018000001",
        "hash": "0x567f446fa306c5d4f784bd5bae202d2de15e62c501ff5fe043a1335d062c9429",
        "nonce": "0x0",
        "number": "0x400",
        "parent_hash": "0x8381df265c9442d5c27559b167892c5a6a8322871112d3cc8ef45222c6624831",
        "proposals_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
        "timestamp": "0x5cd2b117",
        "transactions_root": "0x12214693b8bd5c3d8f96e270dc8fe32b1702bd97630a9eab53a69793e6bc893f",
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
    "result": "0x567f446fa306c5d4f784bd5bae202d2de15e62c501ff5fe043a1335d062c9429"
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
        "0x567f446fa306c5d4f784bd5bae202d2de15e62c501ff5fe043a1335d062c9429"
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
            "dao": "0xb54bdd7f6be90000bb52f392d41cd70024f7ef29b437000000febffacf030000",
            "epoch": "0x7080018000001",
            "hash": "0x567f446fa306c5d4f784bd5bae202d2de15e62c501ff5fe043a1335d062c9429",
            "nonce": "0x0",
            "number": "0x400",
            "parent_hash": "0x8381df265c9442d5c27559b167892c5a6a8322871112d3cc8ef45222c6624831",
            "proposals_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
            "timestamp": "0x5cd2b117",
            "transactions_root": "0x12214693b8bd5c3d8f96e270dc8fe32b1702bd97630a9eab53a69793e6bc893f",
            "uncles_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
            "version": "0x0"
        },
        "proposals": [],
        "transactions": [
            {
                "cell_deps": [],
                "hash": "0xc780f93f92f443ca0b698614afd1e7943e7d4fdc0c2de8dcdea30d4a3fdb02e3",
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
                        "capacity": "0x18ef9705d5",
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
        "0x567f446fa306c5d4f784bd5bae202d2de15e62c501ff5fe043a1335d062c9429"
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
        "dao": "0xb54bdd7f6be90000bb52f392d41cd70024f7ef29b437000000febffacf030000",
        "epoch": "0x7080018000001",
        "hash": "0x567f446fa306c5d4f784bd5bae202d2de15e62c501ff5fe043a1335d062c9429",
        "nonce": "0x0",
        "number": "0x400",
        "parent_hash": "0x8381df265c9442d5c27559b167892c5a6a8322871112d3cc8ef45222c6624831",
        "proposals_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
        "timestamp": "0x5cd2b117",
        "transactions_root": "0x12214693b8bd5c3d8f96e270dc8fe32b1702bd97630a9eab53a69793e6bc893f",
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
        "dao": "0xb54bdd7f6be90000bb52f392d41cd70024f7ef29b437000000febffacf030000",
        "epoch": "0x7080018000001",
        "hash": "0x567f446fa306c5d4f784bd5bae202d2de15e62c501ff5fe043a1335d062c9429",
        "nonce": "0x0",
        "number": "0x400",
        "parent_hash": "0x8381df265c9442d5c27559b167892c5a6a8322871112d3cc8ef45222c6624831",
        "proposals_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
        "timestamp": "0x5cd2b117",
        "transactions_root": "0x12214693b8bd5c3d8f96e270dc8fe32b1702bd97630a9eab53a69793e6bc893f",
        "uncles_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
        "version": "0x0"
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
            "block_hash": "0xb5703168925268fd7ed0712df05418a83344dbdca9fa2b0363d4e79d841421ae",
            "capacity": "0x2effd6e712",
            "lock": {
                "args": "0x",
                "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
                "hash_type": "data"
            },
            "out_point": {
                "index": "0x0",
                "tx_hash": "0x15fd1dcf2763f360085a62dcbb4292057bc04931b16a413d0b6a4b932caf44af"
            }
        },
        {
            "block_hash": "0x9c7ca2051e1d2ee2cf40740ac62500131830d064ab339fede3aba06d02521d41",
            "capacity": "0x2dc7e15ccc",
            "lock": {
                "args": "0x",
                "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
                "hash_type": "data"
            },
            "out_point": {
                "index": "0x0",
                "tx_hash": "0xce4ec7bc6b3ee0533fd38f675926446df4f65d68cb03095d1470446383dbe09b"
            }
        },
        {
            "block_hash": "0x809dc37198dcf7d5cb5201d24cfed2d682f890ce110b6e8d9c315813ace39867",
            "capacity": "0x2d6528a22b",
            "lock": {
                "args": "0x",
                "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
                "hash_type": "data"
            },
            "out_point": {
                "index": "0x0",
                "tx_hash": "0xfc2b1923996ac950ded04a28ea772be6dea37b98c01563019e9928766f3f2f7d"
            }
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
        "0x908a14d1cf5b03e29e5db7e4f550eb3ed2505a1a090a4fb12ef8012f26385777"
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
            "hash": "0x908a14d1cf5b03e29e5db7e4f550eb3ed2505a1a090a4fb12ef8012f26385777",
            "header_deps": [
                "0xb67d20579b685b872dbefcfce82aaedaaa4563fdc2a5cb6b56367db06e094feb"
            ],
            "inputs": [
                {
                    "previous_output": {
                        "index": "0x0",
                        "tx_hash": "0xc780f93f92f443ca0b698614afd1e7943e7d4fdc0c2de8dcdea30d4a3fdb02e3"
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
        "0x567f446fa306c5d4f784bd5bae202d2de15e62c501ff5fe043a1335d062c9429"
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
        "secondary": "0x2104da0b",
        "total": "0x18ef9705d5",
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
            "dao": "0xb54bdd7f6be90000bb52f392d41cd70024f7ef29b437000000febffacf030000",
            "epoch": "0x7080018000001",
            "hash": "0x567f446fa306c5d4f784bd5bae202d2de15e62c501ff5fe043a1335d062c9429",
            "nonce": "0x0",
            "number": "0x400",
            "parent_hash": "0x8381df265c9442d5c27559b167892c5a6a8322871112d3cc8ef45222c6624831",
            "proposals_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
            "timestamp": "0x5cd2b117",
            "transactions_root": "0x12214693b8bd5c3d8f96e270dc8fe32b1702bd97630a9eab53a69793e6bc893f",
            "uncles_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
            "version": "0x0"
        },
        "proposals": [],
        "transactions": [
            {
                "cell_deps": [],
                "hash": "0xc780f93f92f443ca0b698614afd1e7943e7d4fdc0c2de8dcdea30d4a3fdb02e3",
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
                        "capacity": "0x18ef9705d5",
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
                "0xb67d20579b685b872dbefcfce82aaedaaa4563fdc2a5cb6b56367db06e094feb"
            ],
            "inputs": [
                {
                    "previous_output": {
                        "index": "0x0",
                        "tx_hash": "0xc780f93f92f443ca0b698614afd1e7943e7d4fdc0c2de8dcdea30d4a3fdb02e3"
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
                "0xb67d20579b685b872dbefcfce82aaedaaa4563fdc2a5cb6b56367db06e094feb"
            ],
            "inputs": [
                {
                    "previous_output": {
                        "index": "0x0",
                        "tx_hash": "0xc780f93f92f443ca0b698614afd1e7943e7d4fdc0c2de8dcdea30d4a3fdb02e3"
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
    "result": "0x908a14d1cf5b03e29e5db7e4f550eb3ed2505a1a090a4fb12ef8012f26385777"
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
        "0x567f446fa306c5d4f784bd5bae202d2de15e62c501ff5fe043a1335d062c9429"
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
        "block_hash": "0x567f446fa306c5d4f784bd5bae202d2de15e62c501ff5fe043a1335d062c9429",
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
            "block_hash": "0x567f446fa306c5d4f784bd5bae202d2de15e62c501ff5fe043a1335d062c9429",
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
                "capacity": "0x2ce1348802",
                "lock": {
                    "args": "0x",
                    "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
                    "hash_type": "data"
                },
                "type": null
            },
            "created_by": {
                "block_number": "0x98",
                "index": "0x0",
                "tx_hash": "0x72bc04abf9021fdce48ceb0f6bf93444838d99c3e330fd6bbfcaa4a86ed51590"
            }
        },
        {
            "cell_output": {
                "capacity": "0x2ce136743b",
                "lock": {
                    "args": "0x",
                    "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
                    "hash_type": "data"
                },
                "type": null
            },
            "created_by": {
                "block_number": "0x99",
                "index": "0x0",
                "tx_hash": "0x5a47b2f04cbf272ead42ad2d532b6df85d0aa7684d25fcc9eacda94187952373"
            }
        },
        {
            "cell_output": {
                "capacity": "0x2ce1385991",
                "lock": {
                    "args": "0x",
                    "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
                    "hash_type": "data"
                },
                "type": null
            },
            "created_by": {
                "block_number": "0x9a",
                "index": "0x0",
                "tx_hash": "0x9266ea43874f058c1d1fbf3236859c59b4d775d84ecf35fc333a4bb9a9dd7b97"
            }
        },
        {
            "cell_output": {
                "capacity": "0x2ce13a3829",
                "lock": {
                    "args": "0x",
                    "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
                    "hash_type": "data"
                },
                "type": null
            },
            "created_by": {
                "block_number": "0x9b",
                "index": "0x0",
                "tx_hash": "0x114600c93152c671de234f5746f388ba758ba24238b3181e60dd2db9af5a62d8"
            }
        },
        {
            "cell_output": {
                "capacity": "0x2ce13c1025",
                "lock": {
                    "args": "0x",
                    "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
                    "hash_type": "data"
                },
                "type": null
            },
            "created_by": {
                "block_number": "0x9c",
                "index": "0x0",
                "tx_hash": "0x0ad7933beaa38c2407e16f51c7902d07671212212f6fdce5ef561e980f92a2d1"
            }
        },
        {
            "cell_output": {
                "capacity": "0x2ce13de1aa",
                "lock": {
                    "args": "0x",
                    "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
                    "hash_type": "data"
                },
                "type": null
            },
            "created_by": {
                "block_number": "0x9d",
                "index": "0x0",
                "tx_hash": "0x12cbed669294d3cdb156d2a35f1d2452eeac8c25dd35313fcc750eb7cfedaf0f"
            }
        },
        {
            "cell_output": {
                "capacity": "0x2ce13facd9",
                "lock": {
                    "args": "0x",
                    "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
                    "hash_type": "data"
                },
                "type": null
            },
            "created_by": {
                "block_number": "0x9e",
                "index": "0x0",
                "tx_hash": "0xa08caa51ead4828887ce79d9118d3aeb741042e0cebb78ac3c1955fc747dcbec"
            }
        },
        {
            "cell_output": {
                "capacity": "0x2ce14171d2",
                "lock": {
                    "args": "0x",
                    "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
                    "hash_type": "data"
                },
                "type": null
            },
            "created_by": {
                "block_number": "0x9f",
                "index": "0x0",
                "tx_hash": "0x30f0a82c5feef900a7ed6d59d48202996de500994ed2bdca42a46c8269bb59d4"
            }
        },
        {
            "cell_output": {
                "capacity": "0x2ce14330b6",
                "lock": {
                    "args": "0x",
                    "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
                    "hash_type": "data"
                },
                "type": null
            },
            "created_by": {
                "block_number": "0xa0",
                "index": "0x0",
                "tx_hash": "0xf259b6995e443fd0815cc76b328dab1bf2e1c72feaa493320e93a56a71c4f67b"
            }
        },
        {
            "cell_output": {
                "capacity": "0x2ce144e9a4",
                "lock": {
                    "args": "0x",
                    "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
                    "hash_type": "data"
                },
                "type": null
            },
            "created_by": {
                "block_number": "0xa1",
                "index": "0x0",
                "tx_hash": "0x612f4431167a721296a84df0d516451d1897b98cd388956b37b7ea428b5c689b"
            }
        },
        {
            "cell_output": {
                "capacity": "0x2ce1469cba",
                "lock": {
                    "args": "0x",
                    "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
                    "hash_type": "data"
                },
                "type": null
            },
            "created_by": {
                "block_number": "0xa2",
                "index": "0x0",
                "tx_hash": "0x5eb991401219da842a67c770fba18782e5ea1965a31da53e5a70f06510038c31"
            }
        },
        {
            "cell_output": {
                "capacity": "0x2ce1484a15",
                "lock": {
                    "args": "0x",
                    "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
                    "hash_type": "data"
                },
                "type": null
            },
            "created_by": {
                "block_number": "0xa3",
                "index": "0x0",
                "tx_hash": "0x2cd590f35a8252eef8e07e10e6f8be8318e68a113940a96fea77f943539c0635"
            }
        },
        {
            "cell_output": {
                "capacity": "0x2ce149f1d4",
                "lock": {
                    "args": "0x",
                    "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
                    "hash_type": "data"
                },
                "type": null
            },
            "created_by": {
                "block_number": "0xa4",
                "index": "0x0",
                "tx_hash": "0x247d3b0881fd8e3f9b89c6bcecbee770c5ecbdc8c1ee2efeed5d86c99cb90874"
            }
        },
        {
            "cell_output": {
                "capacity": "0x2ce14b9410",
                "lock": {
                    "args": "0x",
                    "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
                    "hash_type": "data"
                },
                "type": null
            },
            "created_by": {
                "block_number": "0xa5",
                "index": "0x0",
                "tx_hash": "0xe3f2faf34c82746ef0ba7aab3a960dc4816ec8bd3f6b20fdf20fc618bc04f01b"
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
                "tx_hash": "0x72bc04abf9021fdce48ceb0f6bf93444838d99c3e330fd6bbfcaa4a86ed51590"
            }
        },
        {
            "consumed_by": null,
            "created_by": {
                "block_number": "0x99",
                "index": "0x0",
                "tx_hash": "0x5a47b2f04cbf272ead42ad2d532b6df85d0aa7684d25fcc9eacda94187952373"
            }
        },
        {
            "consumed_by": null,
            "created_by": {
                "block_number": "0x9a",
                "index": "0x0",
                "tx_hash": "0x9266ea43874f058c1d1fbf3236859c59b4d775d84ecf35fc333a4bb9a9dd7b97"
            }
        },
        {
            "consumed_by": null,
            "created_by": {
                "block_number": "0x9b",
                "index": "0x0",
                "tx_hash": "0x114600c93152c671de234f5746f388ba758ba24238b3181e60dd2db9af5a62d8"
            }
        },
        {
            "consumed_by": null,
            "created_by": {
                "block_number": "0x9c",
                "index": "0x0",
                "tx_hash": "0x0ad7933beaa38c2407e16f51c7902d07671212212f6fdce5ef561e980f92a2d1"
            }
        },
        {
            "consumed_by": null,
            "created_by": {
                "block_number": "0x9d",
                "index": "0x0",
                "tx_hash": "0x12cbed669294d3cdb156d2a35f1d2452eeac8c25dd35313fcc750eb7cfedaf0f"
            }
        },
        {
            "consumed_by": null,
            "created_by": {
                "block_number": "0x9e",
                "index": "0x0",
                "tx_hash": "0xa08caa51ead4828887ce79d9118d3aeb741042e0cebb78ac3c1955fc747dcbec"
            }
        },
        {
            "consumed_by": null,
            "created_by": {
                "block_number": "0x9f",
                "index": "0x0",
                "tx_hash": "0x30f0a82c5feef900a7ed6d59d48202996de500994ed2bdca42a46c8269bb59d4"
            }
        },
        {
            "consumed_by": null,
            "created_by": {
                "block_number": "0xa0",
                "index": "0x0",
                "tx_hash": "0xf259b6995e443fd0815cc76b328dab1bf2e1c72feaa493320e93a56a71c4f67b"
            }
        },
        {
            "consumed_by": null,
            "created_by": {
                "block_number": "0xa1",
                "index": "0x0",
                "tx_hash": "0x612f4431167a721296a84df0d516451d1897b98cd388956b37b7ea428b5c689b"
            }
        },
        {
            "consumed_by": null,
            "created_by": {
                "block_number": "0xa2",
                "index": "0x0",
                "tx_hash": "0x5eb991401219da842a67c770fba18782e5ea1965a31da53e5a70f06510038c31"
            }
        },
        {
            "consumed_by": null,
            "created_by": {
                "block_number": "0xa3",
                "index": "0x0",
                "tx_hash": "0x2cd590f35a8252eef8e07e10e6f8be8318e68a113940a96fea77f943539c0635"
            }
        },
        {
            "consumed_by": null,
            "created_by": {
                "block_number": "0xa4",
                "index": "0x0",
                "tx_hash": "0x247d3b0881fd8e3f9b89c6bcecbee770c5ecbdc8c1ee2efeed5d86c99cb90874"
            }
        },
        {
            "consumed_by": null,
            "created_by": {
                "block_number": "0xa5",
                "index": "0x0",
                "tx_hash": "0xe3f2faf34c82746ef0ba7aab3a960dc4816ec8bd3f6b20fdf20fc618bc04f01b"
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
        "current_time": "0x16d6269e84f",
        "cycles_limit": "0x2540be400",
        "dao": "0x004fb9e277860700b2f80165348723003d1862ec960000000028eb3d7e7a0100",
        "difficulty": "0x100",
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
                "dao": "0xb54bdd7f6be90000bb52f392d41cd70024f7ef29b437000000febffacf030000",
                "epoch": "0x7080018000001",
                "nonce": "0x0",
                "number": "0x400",
                "parent_hash": "0x8381df265c9442d5c27559b167892c5a6a8322871112d3cc8ef45222c6624831",
                "proposals_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
                "timestamp": "0x5cd2b117",
                "transactions_root": "0x12214693b8bd5c3d8f96e270dc8fe32b1702bd97630a9eab53a69793e6bc893f",
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
                            "capacity": "0x18ef9705d5",
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
    "result": "0x567f446fa306c5d4f784bd5bae202d2de15e62c501ff5fe043a1335d062c9429"
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
                "0xb67d20579b685b872dbefcfce82aaedaaa4563fdc2a5cb6b56367db06e094feb"
            ],
            "inputs": [
                {
                    "previous_output": {
                        "index": "0x0",
                        "tx_hash": "0xc780f93f92f443ca0b698614afd1e7943e7d4fdc0c2de8dcdea30d4a3fdb02e3"
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
    "result": "0x908a14d1cf5b03e29e5db7e4f550eb3ed2505a1a090a4fb12ef8012f26385777"
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
