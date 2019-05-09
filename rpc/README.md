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
        "epoch": "8",
        "parent_hash": "0xeda8f89d8be63ac9ab976f3eb3adf634c1d200e3d5ccb889071cbb1df83dcabc",
        "seal": {
            "nonce": "17882382774081951528",
            "proof": "0x131c00009227000084330000e54700002d4e0000cd4f000023510000b2560000715a0000156300006d6700007a740000"
        },
        "timestamp": "1555509433451",
        "transactions_root": "0x6eb5de3f5ed394c3eae59b52996bb62ee6ea92e1b0159cd0866a98a6d6864599",
        "proposals_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
        "uncles_count": 2,
        "uncles_hash": "0x8290616424ad001046d5c3f7c232ffc512dcd57d3420b1e968c3460a69524045",
        "version": 0,
        "witnesses_root": "0x0000000000000000000000000000000000000000000000000000000000000000"
    },
    "id": 2
}
```

### get_current_epoch

Returns the information about the current epoch.

#### Examples

```bash
curl -H 'content-type:application/json' \
    -d '{"id": 2, "jsonrpc": "2.0", "method": "get_current_epoch", "params": []}' \
    http://localhost:8114
```

```json
{
    "jsonrpc": "2.0",
    "result": {
        "block_reward": "5000000000000",
        "difficulty": "0x100",
        "last_block_hash_in_previous_epoch": "0x0000000000000000000000000000000000000000000000000000000000000000",
        "length": "1000",
        "number": "0",
        "remainder_reward": "5000000000000",
        "start_number": "0"
    },
    "id": 2
}
```

### get_epoch_by_number

Return the information corresponding the given epoch number

#### Parameters

    epoch_number - Epoch number

#### Examples

```bash
curl -H 'content-type:application/json' \
    -d '{"id": 2, "jsonrpc": "2.0", "method":"get_epoch_by_number","params": ["1"]}' \
    'http://localhost:8114'
```

```json
{
   "jsonrpc" : "2.0",
   "id" : 2,
   "result" : {
      "last_block_hash_in_previous_epoch" : "0xc50844458e151d5934c99e2be6183f98573632821ae40e0ed87303e24816f3d3",
      "block_reward" : "1736111111111",
      "start_number" : "1000",
      "length" : "2880",
      "difficulty" : "0x100",
      "remainder_reward" : "320",
      "number" : "1"
   }
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
            "epoch": "0",
            "parent_hash": "0xf17b8bfe49aaa018610d20a19aa6a0639882a774c47bcb7623a085a59ee13d42",
            "seal": {
                "nonce": "14785007515249450415",
                "proof": "0xa00600005a0a00001c21000009230000db240000fb350000523600005f4b0000bb4b00000a4d00001b56000070700000"
            },
            "timestamp": "1555422499746",
            "transactions_root": "0xbd9ed8dec5288bdeb2ebbcc4c118a8adb6baab07a44ea79843255ccda6c57915",
            "proposals_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
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

### get_block_by_number

Returns the information about a block by block number (height).

#### Parameters

    number - Number of a block.

#### Examples

```bash
curl -H 'content-type:application/json' \
    -d '{"id": 2, "jsonrpc": "2.0", "method": "get_block_by_number", "params": ["1"]}' \
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
            "proposals_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
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
        "is_outbound": null,
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
            "is_outbound": true,
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
            "is_outbound": false,
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

### dry_run_transaction

Dry run transaction and return the execution cycles.

This method will not check the transaction validaty, but only run the lock script
and type script and than return the execution cycles.

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
   "id" : 2,
   "method" : "dry_run_transaction",
   "jsonrpc" : "2.0",
   "params" : [
      {
         "deps" : [
            {
               "tx_hash" : "0x4974d7ab2d9c548447e12a5488965352bf3d72ef7e6b09445be228be40b73dfe",
               "index" : 0
            }
         ],
         "inputs" : [
            {
               "args" : [
                  "0x303234613530316566643332386530363263383637356632333635393730373238633835396335393262656565666436626538656164336439303133333062633031",
                  "0x33303435303232313030643066313935343561656635633463336565306564633066666439633635303165623864303363393831626236313235623335323130616533666134663635373032323034616331353230333134386364346438373566353436323134343661656437353164326466646339666536653464313431663164306231613763393630613337",
                  "0x31"
               ],
               "since" : "0",
               "previous_output" : {
                  "index" : 1,
                  "tx_hash" : "0x98427f95a11fe26b42e7e29ef9ac14b0b84b0a8359b05fd0824b84e09d41c9ea"
               }
            }
         ],
         "outputs" : [
            {
               "capacity" : "1000000000000",
               "type" : {
                  "args" : [
                     "0x31202b202032202b2033202b20340a",
                     "0x546f6b656e2031",
                     "0x303234613530316566643332386530363263383637356632333635393730373238633835396335393262656565666436626538656164336439303133333062633031"
                  ],
                  "code_hash" : "0x8c3b24e83d111d3a8430416df6a16e33d729273be82cfe0fc994bf147cd8a4ee"
               },
               "data" : "0x80969800000000003044022052de9ce28c28c0c2f8b4819b30d8d936718998808e3d74aca73fdae7bd0904ed022024f9556fa1ea1e8921df0931daf2016f0fb75bbe8bcda5ef625ffb779ad2853c",
               "lock" : {
                  "args" : [
                     "0x31202b202032202b2033202b20340a",
                     "0x546f6b656e2031",
                     "0x303234613530316566643332386530363263383637356632333635393730373238633835396335393262656565666436626538656164336439303133333062633031"
                  ],
                  "code_hash" : "0x8c3b24e83d111d3a8430416df6a16e33d729273be82cfe0fc994bf147cd8a4ee"
               }
            },
            {
               "capacity" : "28000000000000",
               "type" : null,
               "data" : "0x",
               "lock" : {
                  "code_hash" : "0x8c3b24e83d111d3a8430416df6a16e33d729273be82cfe0fc994bf147cd8a4ee",
                  "args" : [
                     "0x31202b2032202b2033202b20340a",
                     "0x65646134626639666336373064656339636663663831333839393966613437353835313731633966636166313162336364616439363939656233633435343766"
                  ]
               }
            }
         ],
         "witnesses" : [],
         "version" : 0
      }
   ]
}' \
    | tr -d '\n' \
    | curl -H 'content-type:application/json' -d @- \
    http://localhost:8114
```

```json
{
   "jsonrpc" : "2.0",
   "id" : 2,
   "result" : {
      "cycles" : "20650838"
   }
}
```

### _compute_transaction_id

Return the transaction id

**Deprecated**: will be removed in a later version

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
   "id" : 2,
   "method" : "_compute_transaction_id",
   "jsonrpc" : "2.0",
   "params" : [
      {
         "deps" : [
            {
               "tx_hash" : "0x4974d7ab2d9c548447e12a5488965352bf3d72ef7e6b09445be228be40b73dfe",
               "index" : 0
            }
         ],
         "inputs" : [
            {
               "args" : [
                  "0x303234613530316566643332386530363263383637356632333635393730373238633835396335393262656565666436626538656164336439303133333062633031",
                  "0x33303435303232313030643066313935343561656635633463336565306564633066666439633635303165623864303363393831626236313235623335323130616533666134663635373032323034616331353230333134386364346438373566353436323134343661656437353164326466646339666536653464313431663164306231613763393630613337",
                  "0x31"
               ],
               "since" : "0",
               "previous_output" : {
                  "index" : 1,
                  "tx_hash" : "0x98427f95a11fe26b42e7e29ef9ac14b0b84b0a8359b05fd0824b84e09d41c9ea"
               }
            }
         ],
         "outputs" : [
            {
               "capacity" : "1000000000000",
               "type" : {
                  "args" : [
                     "0x31202b202032202b2033202b20340a",
                     "0x546f6b656e2031",
                     "0x303234613530316566643332386530363263383637356632333635393730373238633835396335393262656565666436626538656164336439303133333062633031"
                  ],
                  "code_hash" : "0x8c3b24e83d111d3a8430416df6a16e33d729273be82cfe0fc994bf147cd8a4ee"
               },
               "data" : "0x80969800000000003044022052de9ce28c28c0c2f8b4819b30d8d936718998808e3d74aca73fdae7bd0904ed022024f9556fa1ea1e8921df0931daf2016f0fb75bbe8bcda5ef625ffb779ad2853c",
               "lock" : {
                  "args" : [
                     "0x31202b202032202b2033202b20340a",
                     "0x546f6b656e2031",
                     "0x303234613530316566643332386530363263383637356632333635393730373238633835396335393262656565666436626538656164336439303133333062633031"
                  ],
                  "code_hash" : "0x8c3b24e83d111d3a8430416df6a16e33d729273be82cfe0fc994bf147cd8a4ee"
               }
            },
            {
               "capacity" : "28000000000000",
               "type" : null,
               "data" : "0x",
               "lock" : {
                  "code_hash" : "0x8c3b24e83d111d3a8430416df6a16e33d729273be82cfe0fc994bf147cd8a4ee",
                  "args" : [
                     "0x31202b2032202b2033202b20340a",
                     "0x65646134626639666336373064656339636663663831333839393966613437353835313731633966636166313162336364616439363939656233633435343766"
                  ]
               }
            }
         ],
         "witnesses" : [],
         "version" : 0
      }
   ]
}' \
    | tr -d '\n' \
    | curl -H 'content-type:application/json' -d @- \
    http://localhost:8114
```

```json
{
   "jsonrpc" : "2.0",
   "result" : "0x943e5fe84a56fc8bf8f9deaf89d477fcf451e9752379cc9d6996f1fe938c95a7",
   "id" : 2
}
```

### get_blockchain_info

Return state info of blockchain

#### Examples

```bash
curl -H 'content-type:application/json' \
    -d '{"id": 2, "jsonrpc": "2.0", "method": "get_blockchain_info", "params": []}' \
    http://localhost:8114'
```

```json
{
   "result" : {
      "is_initial_block_download" : false,
      "epoch" : "0",
      "difficulty" : "0x100",
      "median_time" : "1557287480008",
      "chain" : "ckb_dev",
      "warnings" : ""
   },
   "id" : 2,
   "jsonrpc" : "2.0"
}
```

### get_peers_state

Return state info of peers

```bash
curl -H 'content-type:application/json' \
    -d '{"id": 2, "jsonrpc": "2.0", "method": "get_peers_state", "params": []}' \
    http://localhost:8114'
```

```json
{
   "result" : [
      {
         "last_updated" : "1557289448237",
         "blocks_in_flight" : "86",
         "peer" : "1"
      }
   ],
   "jsonrpc" : "2.0",
   "id" : 2
}
```
