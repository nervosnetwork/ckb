# CKB JSON-RPC Protocols

## Chain

### get_tip_block_number

Returns the number of blocks in the longest blockchain.

#### Examples

```bash
curl -H 'content-type:application/json' \
    -d '{"id": 2, "jsonrpc": "2.0", "method": "get_tip_block_number", "params": []}' \
    http://localhost:8114
```

```json
{
    "jsonrpc": "2.0",
    "result": "9140",
    "id": 2
}
```

### get_tip_header

Returns the information about the tip header of the longest.

#### Examples

```bash
curl -H 'content-type:application/json' \
    -d '{"id": 2, "jsonrpc": "2.0", "method": "get_tip_header", "params": []}' \
    http://localhost:8114
```

```json
{
    "jsonrpc": "2.0",
    "result": {
        "difficulty": "0x800",
        "hash": "0x80abcbd9395ba17ff9e677d373927adb8519a9fa7bc01d054f6d23584630fb9c",
        "number": "9145",
        "parent_hash": "0xeda8f89d8be63ac9ab976f3eb3adf634c1d200e3d5ccb889071cbb1df83dcabc",
        "seal": {
            "nonce": "17882382774081951528",
            "proof": "0x131c00009227000084330000e54700002d4e0000cd4f000023510000b2560000715a0000156300006d6700007a740000"
        },
        "timestamp": "1555509433451",
        "transactions_root": "0x6eb5de3f5ed394c3eae59b52996bb62ee6ea92e1b0159cd0866a98a6d6864599",
        "proposals_root": "0x0000000000000000000000000000000000000000000000000000000000000000",
        "uncles_count": 2,
        "uncles_hash": "0x8290616424ad001046d5c3f7c232ffc512dcd57d3420b1e968c3460a69524045",
        "version": 0,
        "witnesses_root": "0x0000000000000000000000000000000000000000000000000000000000000000"
    },
    "id": 2
}
```

### get_block_hash

Returns the hash of a block in the best-block-chain by block number; block of No.0 is the genesis block.

#### Parameters

    block_number - Number of a block.

#### Examples

```bash
curl -H 'content-type:application/json' \
    -d '{"id": 2, "jsonrpc": "2.0", "method": "get_block_hash", "params": ["1"]}' \
    http://localhost:8114
```

```json
{
    "jsonrpc": "2.0",
    "result": "0xef285e5da29247ce39385cbd8dc36535f7ea1b5b0379db26e9d459a8b47d0d71",
    "id": 2
}
```

### get_block

Returns the information about a block by hash.

#### Parameters

    hash - Hash of a block.

#### Examples

```bash
curl -H 'content-type:application/json' \
    -d '{"id": 2, "jsonrpc": "2.0", "method": "get_block", "params": ["0xef285e5da29247ce39385cbd8dc36535f7ea1b5b0379db26e9d459a8b47d0d71"]}' \
    http://localhost:8114
```

```json
{
    "jsonrpc": "2.0",
    "result": {
        "transactions": [
            {
                "deps": [],
                "hash": "0xbd9ed8dec5288bdeb2ebbcc4c118a8adb6baab07a44ea79843255ccda6c57915",
                "inputs": [
                    {
                        "args": [
                            "0x0100000000000000"
                        ],
                        "previous_output": {
                            "tx_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
                            "index": 4294967295
                        },
                        "since": "0"
                    }
                ],
                "outputs": [
                    {
                        "capacity": "50000",
                        "data": "0x",
                        "lock": {
                            "args": [],
                            "code_hash": "0x0000000000000000000000000000000000000000000000000000000000000001"
                        },
                        "type": null
                    }
                ],
                "version": 0,
                "witnesses": []
            }
        ],
        "header": {
            "difficulty": "0x100",
            "hash": "0xef285e5da29247ce39385cbd8dc36535f7ea1b5b0379db26e9d459a8b47d0d71",
            "number": "1",
            "parent_hash": "0xf17b8bfe49aaa018610d20a19aa6a0639882a774c47bcb7623a085a59ee13d42",
            "seal": {
                "nonce": "14785007515249450415",
                "proof": "0xa00600005a0a00001c21000009230000db240000fb350000523600005f4b0000bb4b00000a4d00001b56000070700000"
            },
            "timestamp": "1555422499746",
            "transactions_root": "0xbd9ed8dec5288bdeb2ebbcc4c118a8adb6baab07a44ea79843255ccda6c57915",
            "proposals_root": "0x0000000000000000000000000000000000000000000000000000000000000000",
            "uncles_count": 0,
            "uncles_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
            "version": 0,
            "witnesses_root": "0x0000000000000000000000000000000000000000000000000000000000000000"
        },
        "proposals": [],
        "uncles": []
    },
    "id": 2
}
```

### get_transaction

Returns the information about a transaction requested by transaction hash.

#### Parameters

    hash - Hash of a transaction.

#### Examples

```bash
curl -H 'content-type:application/json' \
    -d '{"id": 2, "jsonrpc": "2.0", "method": "get_transaction", "params": ["0xa093b2e820f3f2202a6802314ece2eee3f863b177b3abe11bf16b1588152d31b"]}' \
    http://localhost:8114'
```

```json
{
    "jsonrpc": "2.0",
    "result": {
        "transaction" : {
            "deps": [],
            "hash": "0xa093b2e820f3f2202a6802314ece2eee3f863b177b3abe11bf16b1588152d31b",
            "inputs": [
                {
                    "args": [],
                    "previous_output": {
                        "tx_hash": "0xeea31bfdcc4ac3bcb0204c450f08fb46c3840042b0a4e657edff3180cbb01c47",
                        "index": 2996
                    },
                    "since": "0"
                }
            ],
            "outputs": [
                {
                    "capacity": "1000",
                    "data": "0x",
                    "lock": {
                        "args": [
                            "0x79616e676279"
                        ],
                        "code_hash": "0x0000000000000000000000000000000000000000000000000000000000000001"
                    },
                    "type": null
                }
            ],
            "version": 0,
            "witnesses": []
        },
        "tx_status": {
            "status": "committed",
            "block_hash": "0xef285e5da29247ce39385cbd8dc36535f7ea1b5b0379db26e9d459a8b47d0d71"
        }
    },
    "id": 2
}
```

#### `tx_status` Possible Values

```
{
    "tx_status": {
        "status": "pending",
        "block_hash": null
    }
}

{
    "tx_status": {
        "status": "proposed",
        "block_hash": null
    }
}

{
    "tx_status": {
        "status": "committed",
        "block_hash": "0xef285e5da29247ce39385cbd8dc36535f7ea1b5b0379db26e9d459a8b47d0d71"
    }
}
```

### get_cells_by_lock_hash

Returns the information about cells collection by the hash of lock script.

#### Parameters

    lock_hash - Cell lock script hash.
    from - Start block number.
    to - End block number.

#### Examples

```bash
curl -H 'content-type:application/json' \
    -d '{"id": 2, "jsonrpc": "2.0", "method": "get_cells_by_lock_hash", "params": ["0xcb7bce98a778f130d34da522623d7e56705bddfe0dc4781bd2331211134a19a5", "9001", "9003"]}' \
    http://localhost:8114
```

```json
{
    "jsonrpc": "2.0",
    "result": [
        {
            "capacity": 50000,
            "lock": {
                "args": [],
                "code_hash": "0x0000000000000000000000000000000000000000000000000000000000000001"
            },
            "out_point": {
                "tx_hash": "0xc15274f7aaec78b74ea2b87a2aefd5dc3e003b367eab326a29a73900fd9b91ff",
                "index": 0
            }
        },
        {
            "capacity": 50000,
            "lock": {
                "args": [],
                "code_hash": "0x0000000000000000000000000000000000000000000000000000000000000001"
            },
            "out_point": {
                "tx_hash": "0xbcc4ffd86c681c1004f746422e33b1ac3cd59bdf6155afd5ea076219ed29bbae",
                "index": 0
            }
        },
        {
            "capacity": 50000,
            "lock": {
                "args": [],
                "code_hash": "0x0000000000000000000000000000000000000000000000000000000000000001"
            },
            "out_point": {
                "tx_hash": "0x9289e12f0a9b2cfce51cd4a64d733c0a3ca9a52093669863c485ea6dfae81a3e",
                "index": 0
            }
        }
    ],
    "id": 2
}
```

### get_live_cell

Returns the information about a cell by out_point.

#### Parameters

    out_point - OutPoint object {"tx_hash": <tx_hash>, "index": <index>}.

#### Examples

```bash
curl -H 'content-type:application/json' \
    -d '{"id": 2, "jsonrpc": "2.0", "method":"get_live_cell","params": [{"tx_hash": "0xbcc4ffd86c681c1004f746422e33b1ac3cd59bdf6155afd5ea076219ed29bbae", "index": 0}]}' \
    http://localhost:8114
```

```json
{
    "jsonrpc": "2.0",
    "result": {
        "cell": {
            "capacity": "50000",
            "data": "0x",
            "lock": {
                "args": [],
                "code_hash": "0x0000000000000000000000000000000000000000000000000000000000000001"
            },
            "type": null
        },
        "status": "live"
    },
    "id": 2
}
```

## Net

### local_node_info

Returns the local node information.

#### Examples

```bash
curl -H 'content-type:application/json' \
    -d '{"id": 2, "jsonrpc": "2.0", "method": "local_node_info", "params": []}' \
    http://localhost:8114
```

```json
{
    "jsonrpc": "2.0",
    "result": {
        "addresses": [
            {
                "address": "/ip4/192.168.0.2/tcp/8112/p2p/QmTRHCdrRtgUzYLNCin69zEvPvLYdxUZLLfLYyHVY3DZAS",
                "score": 255
            },
            {
                "address": "/ip4/0.0.0.0/tcp/8112/p2p/QmTRHCdrRtgUzYLNCin69zEvPvLYdxUZLLfLYyHVY3DZAS",
                "score": 1
            }
        ],
        "node_id": "QmTRHCdrRtgUzYLNCin69zEvPvLYdxUZLLfLYyHVY3DZAS",
        "version": "0.9.0"
    },
    "id": 2
}
```

### get_peers

Returns the connected peers information.

#### Examples

```bash
curl -H 'content-type:application/json' \
    -d '{"id": 2, "jsonrpc": "2.0", "method": "get_peers", "params": []}' \
    http://localhost:8114
```

```json
{
    "jsonrpc": "2.0",
    "result": [
        {
            "addresses": [
                {
                    "address": "/ip4/192.168.0.3/tcp/8115",
                    "score": 1
                }
            ],
            "node_id": "QmaaaLB4uPyDpZwTQGhV63zuYrKm4reyN2tF1j2ain4oE7",
            "version": "unknown"
        },
        {
            "addresses": [
                {
                    "address": "/ip4/192.168.0.4/tcp/8113",
                    "score": 255
                }
            ],
            "node_id": "QmRuGcpVC3vE7aEoB6fhUdq9uzdHbyweCnn1sDBSjfmcbM",
            "version": "unknown"
        },
        {
            "addresses": [],
            "node_id": "QmUddxwRqgTmT6tFujXbYPMLGLAE2Tciyv6uHGfdYFyDVa",
            "version": "unknown"
        }
    ],
    "id": 2
}
```

## Pool

### send_transaction

Creates new transaction.

#### Parameters

transaction - The transaction object.

    version - Transaction version.
    deps - Dependent cells.
    inputs - Transaction inputs.
    outputs - Transaction outputs.
    witnesses - Witnesses.

#### Examples

```bash
echo '{
        "id": 2,
        "jsonrpc": "2.0",
        "method": "send_transaction",
        "params": [
            {
                "version": 0,
                "deps": [],
                "inputs": [
                    {
                        "previous_output": {
                            "tx_hash": "0xeea31bfdcc4ac3bcb0204c450f08fb46c3840042b0a4e657edff3180cbb01c47",
                            "index": 2995
                        },
                        "since": "0",
                        "args": []
                    }
                ],
                "outputs": [
                    {
                        "capacity": "1000",
                        "data": "0x",
                        "lock": {
                            "args": [
                                "0x79616e676279"
                            ],
                            "code_hash": "0x0000000000000000000000000000000000000000000000000000000000000001"
                        },
                        "type": null
                    }
                ],
                "witnesses": [],
                "hash": "0x0000000000000000000000000000000000000000000000000000000000000000"
            }
        ]
    }' \
    | tr -d '\n' \
    | curl -H 'content-type:application/json' -d @- \
    http://localhost:8114
```

```json
{
    "jsonrpc": "2.0",
    "result": "0xee577cd94b1f2f1667316ff3cb44810902fd35cf901db28cde955b82eea56725",
    "id": 2
}
```

### tx_pool_info

Return the transaction pool information

#### Examples

``` bash
curl -H 'content-type:application/json' \
    -d '{"params": [], "method": "tx_pool_info", "jsonrpc": "2.0", "id": 2}' \
    http://localhost:8114
```

``` json
{
    "jsonrpc": "2.0",
    "id": 2,
    "result": {
        "pending": 34,
        "staging": 22,
        "orphan": 33,
        "last_txs_updated_at": "1555507787683"
    }
}
```


## Trace

### trace_transaction

Registers a transaction trace, returning the transaction hash.

#### Parameters

transaction - The transaction object.

    version - Transaction version.
    deps - Dependent cells.
    inputs - Transaction inputs.
    outputs - Transaction outputs.
    witnesses - Witnesses.

#### Examples

```bash
echo '{
        "id": 2,
        "jsonrpc": "2.0",
        "method": "trace_transaction",
        "params": [
            {
                "version": 0,
                "deps": [],
                "inputs": [
                    {
                        "previous_output": {
                            "tx_hash": "0xeea31bfdcc4ac3bcb0204c450f08fb46c3840042b0a4e657edff3180cbb01c47",
                            "index": 2996
                        },
                        "since": "0",
                        "args": []
                    }
                ],
                "outputs": [
                    {
                        "capacity": "1000",
                        "data": "0x",
                        "lock": {
                            "args": [
                                "0x79616e676279"
                            ],
                            "code_hash": "0x0000000000000000000000000000000000000000000000000000000000000001"
                        },
                        "type": null
                    }
                ],
                "witnesses": [],
                "hash": "0x0000000000000000000000000000000000000000000000000000000000000000"
            }
        ]
    }' \
    | tr -d '\n' \
    | curl -H 'content-type:application/json' -d @- \
    http://localhost:8114
```

```json
{
    "jsonrpc": "2.0",
    "result": "0xa093b2e820f3f2202a6802314ece2eee3f863b177b3abe11bf16b1588152d31b",
    "id": 2
}
```

### get_transaction_trace

Returns the traces of the transaction submitted by `trace_transaction`.

#### Parameters

    hash - Hash of a transaction.

#### Examples

```bash
curl -H 'content-type:application/json' \
    -d '{"id": 2, "jsonrpc": "2.0", "method": "get_transaction_trace", "params": ["0xa093b2e820f3f2202a6802314ece2eee3f863b177b3abe11bf16b1588152d31b"]}' \
    http://localhost:8114'
```

```json
{
    "jsonrpc": "2.0",
    "result": [
        {
            "action": "AddPending",
            "info": "unknown tx, insert to pending queue",
            "time": 1555507787683
        },
        {
            "action": "Proposed",
            "info": "ProposalShortId(0xa093b2e820f3f2202a68) proposed",
            "time": 1555507857772
        },
        {
            "action": "Staged",
            "info": "tx staged",
            "time": 1555507857782
        },
        {
            "action": "Committed",
            "info": "tx committed",
            "time": 1555507913089
        },
        {
            "action": "Committed",
            "info": "tx committed",
            "time": 1555508028264
        }
    ],
    "id": 2
}
```
