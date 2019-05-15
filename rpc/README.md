# CKB JSON-RPC Protocols


## JSON-RPC

* [`_compute_code_hash`](#_compute_code_hash)
* [`_compute_script_hash`](#_compute_script_hash)
* [`_compute_transaction_hash`](#_compute_transaction_hash)
* [`dry_run_transaction`](#dry_run_transaction)
* [`get_block`](#get_block)
* [`get_block_by_number`](#get_block_by_number)
* [`get_block_hash`](#get_block_hash)
* [`get_blockchain_info`](#get_blockchain_info)
* [`get_cells_by_lock_hash`](#get_cells_by_lock_hash)
* [`get_current_epoch`](#get_current_epoch)
* [`get_epoch_by_number`](#get_epoch_by_number)
* [`get_live_cell`](#get_live_cell)
* [`get_peers`](#get_peers)
* [`get_peers_state`](#get_peers_state)
* [`get_tip_block_number`](#get_tip_block_number)
* [`get_tip_header`](#get_tip_header)
* [`get_transaction`](#get_transaction)
* [`local_node_info`](#local_node_info)
* [`send_transaction`](#send_transaction)
* [`tx_pool_info`](#tx_pool_info)

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
| curl -H 'content-type:application/json' -d @- \
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
            "code_hash": "0xb35557e7e9854206f7bc13e3c3a7fa4cf8892c84a09237fb0aab40aab3771eee"
        }
    ]
}' \
| tr -d '\n' \
| curl -H 'content-type:application/json' -d @- \
http://localhost:8114
```

```json
{
    "id": 2,
    "jsonrpc": "2.0",
    "result": "0x7c72a3b5705bf5a4e7364fc358e2972f4eb376cf7937bf7ffd319f50f07e27a2"
}
```

### `_compute_transaction_hash`

Return the transaction id

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
                        "block_hash": "0xca97a033aa9765914bc0b7887ff8723037614df11d06783555ff97e9a0e72b55",
                        "cell": {
                            "index": "0",
                            "tx_hash": "0x602b713fac38c0224bc2e8ba0ec9c8363d7d1e508897b33c32f45d7554883c81"
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
                        "code_hash": "0x0000000000000000000000000000000000000000000000000000000000000001"
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
| curl -H 'content-type:application/json' -d @- \
http://localhost:8114
```

```json
{
    "id": 2,
    "jsonrpc": "2.0",
    "result": "0xbecff6ee8cf2a40bdf391a03df501791b7946cea9a8d83b3d328a1ea5e1bc000"
}
```

### `dry_run_transaction`

Dry run transaction and return the execution cycles. 

This method will not check the transaction validaty, but only run the lock script 
and type script and than return the execution cycles.


#### Examples

```bash
echo '{
    "id": 2,
    "jsonrpc": "2.0",
    "method": "dry_run_transaction",
    "params": [
        {
            "deps": [],
            "inputs": [
                {
                    "args": [],
                    "previous_output": {
                        "block_hash": "0xca97a033aa9765914bc0b7887ff8723037614df11d06783555ff97e9a0e72b55",
                        "cell": {
                            "index": "0",
                            "tx_hash": "0x602b713fac38c0224bc2e8ba0ec9c8363d7d1e508897b33c32f45d7554883c81"
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
                        "code_hash": "0x0000000000000000000000000000000000000000000000000000000000000001"
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
| curl -H 'content-type:application/json' -d @- \
http://localhost:8114
```

```json
{
    "id": 2,
    "jsonrpc": "2.0",
    "result": {
        "cycles": "0"
    }
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
        "0xa5224507f309ea8e98b5af50cb21e9e957845c6faef730971292df974f6a4e23"
    ]
}' \
| tr -d '\n' \
| curl -H 'content-type:application/json' -d @- \
http://localhost:8114
```

```json
{
    "id": 2,
    "jsonrpc": "2.0",
    "result": {
        "header": {
            "difficulty": "0x3e8",
            "epoch": "0",
            "hash": "0xa5224507f309ea8e98b5af50cb21e9e957845c6faef730971292df974f6a4e23",
            "number": "2",
            "parent_hash": "0x47f672dde8811ea859378444b400f80db7c7c3f73dbd35083506c1e46f924858",
            "proposals_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
            "seal": {
                "nonce": "0",
                "proof": "0x"
            },
            "timestamp": "1557310745",
            "transactions_root": "0x6dc4b2e7b2ddafa108cb38069e1b3a69aead9f27099e7fb50ed935009ec0d397",
            "uncles_count": "0",
            "uncles_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
            "version": "0",
            "witnesses_root": "0x0000000000000000000000000000000000000000000000000000000000000000"
        },
        "proposals": [],
        "transactions": [
            {
                "deps": [],
                "hash": "0x6dc4b2e7b2ddafa108cb38069e1b3a69aead9f27099e7fb50ed935009ec0d397",
                "inputs": [
                    {
                        "args": [
                            "0x0200000000000000"
                        ],
                        "previous_output": {
                            "block_hash": null,
                            "cell": null
                        },
                        "since": "0"
                    }
                ],
                "outputs": [
                    {
                        "capacity": "50000000000000",
                        "data": "0x",
                        "lock": {
                            "args": [],
                            "code_hash": "0x0000000000000000000000000000000000000000000000000000000000000001"
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
| curl -H 'content-type:application/json' -d @- \
http://localhost:8114
```

```json
{
    "id": 2,
    "jsonrpc": "2.0",
    "result": {
        "header": {
            "difficulty": "0x3e8",
            "epoch": "1",
            "hash": "0xca97a033aa9765914bc0b7887ff8723037614df11d06783555ff97e9a0e72b55",
            "number": "1024",
            "parent_hash": "0x574d0233963f188b32e8c791a5d063d96c7f4dd07a3eec9267120806ffd979d5",
            "proposals_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
            "seal": {
                "nonce": "0",
                "proof": "0x"
            },
            "timestamp": "1557311767",
            "transactions_root": "0x602b713fac38c0224bc2e8ba0ec9c8363d7d1e508897b33c32f45d7554883c81",
            "uncles_count": "0",
            "uncles_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
            "version": "0",
            "witnesses_root": "0x0000000000000000000000000000000000000000000000000000000000000000"
        },
        "proposals": [],
        "transactions": [
            {
                "deps": [],
                "hash": "0x602b713fac38c0224bc2e8ba0ec9c8363d7d1e508897b33c32f45d7554883c81",
                "inputs": [
                    {
                        "args": [
                            "0x0004000000000000"
                        ],
                        "previous_output": {
                            "block_hash": null,
                            "cell": null
                        },
                        "since": "0"
                    }
                ],
                "outputs": [
                    {
                        "capacity": "50000000000000",
                        "data": "0x",
                        "lock": {
                            "args": [],
                            "code_hash": "0x0000000000000000000000000000000000000000000000000000000000000001"
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
| curl -H 'content-type:application/json' -d @- \
http://localhost:8114
```

```json
{
    "id": 2,
    "jsonrpc": "2.0",
    "result": "0xa5224507f309ea8e98b5af50cb21e9e957845c6faef730971292df974f6a4e23"
}
```

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
| curl -H 'content-type:application/json' -d @- \
http://localhost:8114
```

```json
{
    "id": 2,
    "jsonrpc": "2.0",
    "result": {
        "chain": "main",
        "difficulty": "0x3e8",
        "epoch": "1",
        "is_initial_block_download": true,
        "median_time": "1557311762",
        "warnings": ""
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
        "0xcb7bce98a778f130d34da522623d7e56705bddfe0dc4781bd2331211134a19a5",
        "2",
        "5"
    ]
}' \
| tr -d '\n' \
| curl -H 'content-type:application/json' -d @- \
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
                "code_hash": "0x0000000000000000000000000000000000000000000000000000000000000001"
            },
            "out_point": {
                "block_hash": null,
                "cell": {
                    "index": "0",
                    "tx_hash": "0x6dc4b2e7b2ddafa108cb38069e1b3a69aead9f27099e7fb50ed935009ec0d397"
                }
            }
        },
        {
            "capacity": "50000000000000",
            "lock": {
                "args": [],
                "code_hash": "0x0000000000000000000000000000000000000000000000000000000000000001"
            },
            "out_point": {
                "block_hash": null,
                "cell": {
                    "index": "0",
                    "tx_hash": "0xc7fb8d25f1f52f7cf359d3605e0bb2ceb4774b4062278a557bebdd3d4bbd89d1"
                }
            }
        },
        {
            "capacity": "50000000000000",
            "lock": {
                "args": [],
                "code_hash": "0x0000000000000000000000000000000000000000000000000000000000000001"
            },
            "out_point": {
                "block_hash": null,
                "cell": {
                    "index": "0",
                    "tx_hash": "0xff5a36107851d244e1543821f9f039c3d4eb69d9968750b0b0e82e78da86c987"
                }
            }
        },
        {
            "capacity": "50000000000000",
            "lock": {
                "args": [],
                "code_hash": "0x0000000000000000000000000000000000000000000000000000000000000001"
            },
            "out_point": {
                "block_hash": null,
                "cell": {
                    "index": "0",
                    "tx_hash": "0x6c59c9628bc8473a1fd61ebac23f061e298a766b407aa300a299c746bdce2f6d"
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
| curl -H 'content-type:application/json' -d @- \
http://localhost:8114
```

```json
{
    "id": 2,
    "jsonrpc": "2.0",
    "result": {
        "block_reward": "173611111111",
        "difficulty": "0x3e8",
        "last_block_hash_in_previous_epoch": "0x3577b8bf75a98d563807542a1ab7cff84b9e9d5f1e0f5757fc5c7b712a629701",
        "length": "2880",
        "number": "1",
        "remainder_reward": "320",
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
| curl -H 'content-type:application/json' -d @- \
http://localhost:8114
```

```json
{
    "id": 2,
    "jsonrpc": "2.0",
    "result": {
        "block_reward": "500000000000",
        "difficulty": "0x3e8",
        "last_block_hash_in_previous_epoch": "0x0000000000000000000000000000000000000000000000000000000000000000",
        "length": "1000",
        "number": "0",
        "remainder_reward": "0",
        "start_number": "0"
    }
}
```

### `get_live_cell`

Returns the information about a cell by out_point.

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
            "block_hash": null,
            "cell": {
                "index": "0",
                "tx_hash": "0xff5a36107851d244e1543821f9f039c3d4eb69d9968750b0b0e82e78da86c987"
            }
        }
    ]
}' \
| tr -d '\n' \
| curl -H 'content-type:application/json' -d @- \
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
                "code_hash": "0x0000000000000000000000000000000000000000000000000000000000000001"
            },
            "type": null
        },
        "status": "live"
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
| curl -H 'content-type:application/json' -d @- \
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

### `get_peers_state`

Return state info of peers


#### Examples

```bash
echo '{
    "id": 2,
    "jsonrpc": "2.0",
    "method": "get_peers_state",
    "params": []
}' \
| tr -d '\n' \
| curl -H 'content-type:application/json' -d @- \
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
| curl -H 'content-type:application/json' -d @- \
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
| curl -H 'content-type:application/json' -d @- \
http://localhost:8114
```

```json
{
    "id": 2,
    "jsonrpc": "2.0",
    "result": {
        "difficulty": "0x3e8",
        "epoch": "1",
        "hash": "0xca97a033aa9765914bc0b7887ff8723037614df11d06783555ff97e9a0e72b55",
        "number": "1024",
        "parent_hash": "0x574d0233963f188b32e8c791a5d063d96c7f4dd07a3eec9267120806ffd979d5",
        "proposals_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
        "seal": {
            "nonce": "0",
            "proof": "0x"
        },
        "timestamp": "1557311767",
        "transactions_root": "0x602b713fac38c0224bc2e8ba0ec9c8363d7d1e508897b33c32f45d7554883c81",
        "uncles_count": "0",
        "uncles_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
        "version": "0",
        "witnesses_root": "0x0000000000000000000000000000000000000000000000000000000000000000"
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
        "0xbecff6ee8cf2a40bdf391a03df501791b7946cea9a8d83b3d328a1ea5e1bc000"
    ]
}' \
| tr -d '\n' \
| curl -H 'content-type:application/json' -d @- \
http://localhost:8114
```

```json
{
    "id": 2,
    "jsonrpc": "2.0",
    "result": {
        "transaction": {
            "deps": [],
            "hash": "0xbecff6ee8cf2a40bdf391a03df501791b7946cea9a8d83b3d328a1ea5e1bc000",
            "inputs": [
                {
                    "args": [],
                    "previous_output": {
                        "block_hash": "0xca97a033aa9765914bc0b7887ff8723037614df11d06783555ff97e9a0e72b55",
                        "cell": {
                            "index": "0",
                            "tx_hash": "0x602b713fac38c0224bc2e8ba0ec9c8363d7d1e508897b33c32f45d7554883c81"
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
                        "code_hash": "0x0000000000000000000000000000000000000000000000000000000000000001"
                    },
                    "type": null
                }
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
| curl -H 'content-type:application/json' -d @- \
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

### `send_transaction`

Send new transaction into transaction pool

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
            "deps": [],
            "inputs": [
                {
                    "args": [],
                    "previous_output": {
                        "block_hash": "0xca97a033aa9765914bc0b7887ff8723037614df11d06783555ff97e9a0e72b55",
                        "cell": {
                            "index": "0",
                            "tx_hash": "0x602b713fac38c0224bc2e8ba0ec9c8363d7d1e508897b33c32f45d7554883c81"
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
                        "code_hash": "0x0000000000000000000000000000000000000000000000000000000000000001"
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
| curl -H 'content-type:application/json' -d @- \
http://localhost:8114
```

```json
{
    "id": 2,
    "jsonrpc": "2.0",
    "result": "0xbecff6ee8cf2a40bdf391a03df501791b7946cea9a8d83b3d328a1ea5e1bc000"
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
| curl -H 'content-type:application/json' -d @- \
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
        "proposed": "0"
    }
}
```
