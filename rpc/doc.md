# get_block

Returns the information about a block by hash.

## Parameters

    Hash - Hash of a block.

## Examples

```shell
curl -d '{"id": 2, "jsonrpc": "2.0", "method":"get_block","params": ["0x7643567cc0b8637505cce071ae764bc17a1d4e37579769c9a863d25841e48a07"]}' -H 'content-type:application/json' 'http://localhost:8114'
```

```json
{
    "jsonrpc": "2.0",
    "result": {
        "hash": "0x7643567cc0b8637505cce071ae764bc17a1d4e37579769c9a863d25841e48a07",
        "header": {
            "raw": {
                "cellbase_id": "0xbddb7c2559c2c3cdfc8f3cae2697ca75489521c352265cc9e60b4b2416ad5929",
                "difficulty": "0x100",
                "number": 1,
                "parent_hash": "0x9b0bd5be9498a0b873d08e242fff306eec04fac7c59ce479b49ca92a8f649982",
                "timestamp": 1544599720510,
                "txs_commit": "0xbddb7c2559c2c3cdfc8f3cae2697ca75489521c352265cc9e60b4b2416ad5929",
                "txs_proposal": "0x0000000000000000000000000000000000000000000000000000000000000000",
                "uncles_count": 0,
                "uncles_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
                "version": 0
            },
            "seal": {
                "nonce": 10545468520399447721,
                "proof": [163, 13, 0, 0, 12, 17, 0, 0, 98, 28, 0, 0, 240, 60, 0, 0, 200, 62, 0, 0, 12, 76, 0, 0, 6, 93, 0, 0, 247, 93, 0, 0, 107, 97, 0, 0, 230, 100, 0, 0, 16, 103, 0, 0, 244, 107, 0, 0],
            }
        },
        "transactions": [
            {
                "hash": "0xbddb7c2559c2c3cdfc8f3cae2697ca75489521c352265cc9e60b4b2416ad5929",
                "transaction": {
                    "deps": [],
                    "inputs": [
                        {
                            "previous_output": {
                                "hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
                                "index": 4294967295
                            },
                            "unlock": {
                                "args": [],
                                "binary": [1, 0, 0, 0, 0, 0, 0, 0],
                                "reference": null,
                                "signed_args": [],
                                "version": 0
                            }
                        }
                    ],
                    "outputs": [
                        {
                            "capacity": 50000,
                            "type": null,
                            "data": [],
                            "lock": "0x321c1ca2887fb8eddaaa7e917399f71e63e03a1c83ff75ed12099a01115ea2ff"
                        }
                    ],
                    "version": 0
                }
            }
        ]
    },
    "id": 2
}
```

# get_transaction

Returns the information about a transaction requested by transaction hash.

## Parameters

    Hash - Hash of a transaction.

## Examples

```shell
curl -d '{"id": 2, "jsonrpc": "2.0", "method":"get_transaction","params": ["0xbddb7c2559c2c3cdfc8f3cae2697ca75489521c352265cc9e60b4b2416ad5929"]}' -H 'content-type:application/json' 'http://localhost:8114'
```

```json
{
    "jsonrpc": "2.0",
    "result": {
        "hash": "0xbddb7c2559c2c3cdfc8f3cae2697ca75489521c352265cc9e60b4b2416ad5929",
        "transaction": {
            "deps": [],
            "inputs": [
                {
                    "previous_output": {
                        "hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
                        "index": 4294967295
                    },
                    "unlock": {
                        "args": [],
                        "binary": [1, 0, 0, 0, 0, 0, 0, 0],
                        "reference": null,
                        "signed_args": [],
                        "version": 0
                    }
                }
            ],
            "outputs": [
                {
                    "capacity": 50000,
                    "type": null,
                    "data": [],
                    "lock": "0x321c1ca2887fb8eddaaa7e917399f71e63e03a1c83ff75ed12099a01115ea2ff"
                }
            ],
            "version": 0
        }
    },
    "id": 2
}
```


# get_block_hash

Returns the hash of a block in the best-block-chain by block number; Block of No. 0 is the genesis block.

## Parameters

    BlockNumber - Number of a block.

## Examples

```shell
curl -d '{"id": 2, "jsonrpc": "2.0", "method":"get_block_hash","params": [1]}' -H 'content-type:application/json' 'http://localhost:8114'
```

```json
{
    "jsonrpc": "2.0",
    "result": "0x7643567cc0b8637505cce071ae764bc17a1d4e37579769c9a863d25841e48a07",
    "id": 2
}
```

# get_tip_header

Returns the information about the tip header of the longest.

## Examples

```shell
curl -d '{"id": 2, "jsonrpc": "2.0", "method":"get_tip_header","params": []}' -H 'content-type:application/json' 'http://localhost:8114'
```

```json
{
    "jsonrpc": "2.0",
    "result": {
        "raw": {
            "cellbase_id": "0xfa531584947a905ca77ce4333bf49c9f20c97b4f464d2545584df946b3803a8b",
            "difficulty": "0x100",
            "number": 1246,
            "parent_hash": "0xcacfcad12f3314138e929ac9833cb956fb42e4531ea5458c6a3c25fefeade315",
            "timestamp": 1544600340130,
            "txs_commit": "0xfa531584947a905ca77ce4333bf49c9f20c97b4f464d2545584df946b3803a8b",
            "txs_proposal": "0x0000000000000000000000000000000000000000000000000000000000000000",
            "uncles_count": 0,
            "uncles_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
            "version": 0
        },
        "seal": {
            "nonce": 15789531258138942821,
            "proof": [130, 6, 0, 0, 187, 6, 0, 0, 14, 8, 0, 0, 219, 18, 0, 0, 94, 39, 0, 0, 108, 70, 0, 0, 234, 71, 0, 0, 9, 75, 0, 0, 19, 91, 0, 0, 122, 96, 0, 0, 253, 98, 0, 0, 249, 121, 0, 0]
        }
    },
    "id": 2
}
```

# get_cells_by_type_hash

Returns the information about cells collection by type_hash.

## Parameters

    Type_hash - Cell type_hash.
    From - Start BlockNumber.
    To - End BlockNumber.

## Examples

```shell
curl -d '{"id": 2, "jsonrpc": "2.0", "method":"get_cells_by_type_hash","params": ["0x321c1ca2887fb8eddaaa7e917399f71e63e03a1c83ff75ed12099a01115ea2ff", 1, 5]}' -H 'content-type:application/json' 'http://localhost:8114'
```

```json
{
    "jsonrpc": "2.0",
    "result": [
        {
            "capacity": 50000,
            "lock": "0x321c1ca2887fb8eddaaa7e917399f71e63e03a1c83ff75ed12099a01115ea2ff",
            "out_point": {
                "hash": "0xbddb7c2559c2c3cdfc8f3cae2697ca75489521c352265cc9e60b4b2416ad5929",
                "index": 0
            }
        },
        {
            "capacity": 50000,
            "lock": "0x321c1ca2887fb8eddaaa7e917399f71e63e03a1c83ff75ed12099a01115ea2ff",
            "out_point": {
                "hash": "0x2c40a96684a99f720b6ab0eeb39564285742c5a2bed12347cd13e6ae50782111",
                "index": 0
            }
        },
        {
            "capacity": 50000,
            "lock": "0x321c1ca2887fb8eddaaa7e917399f71e63e03a1c83ff75ed12099a01115ea2ff",
            "out_point": {
                "hash": "0x1954a9cbb21bebd859260bf851be9f1706b6e25ca511800ea05059f26973ea78",
                "index": 0
            }
        },
        {
            "capacity": 50000,
            "lock": "0x321c1ca2887fb8eddaaa7e917399f71e63e03a1c83ff75ed12099a01115ea2ff",
            "out_point": {
                "hash": "0xc0dc6c4556ee84a176aa6f65493c31ea35d4ee190fe2f2b62b744f347b816d9b",
                "index": 0
            }
        },
        {
            "capacity": 50000,
            "lock": "0x321c1ca2887fb8eddaaa7e917399f71e63e03a1c83ff75ed12099a01115ea2ff",
            "out_point": {
                "hash": "0xa185f069dbebf159bd0dbd495ae0822a9b71d20e59f790b0c814697609257f34",
                "index": 0
            }
        }
    ],
    "id": 2
}
```


# get_live_cell

Returns the information about a cell by out_point.

## Parameters

    OutPoint - OutPoint Object {"hash": <hash>, "index": <index>}.

## Examples

```shell
curl -d '{"id": 2, "jsonrpc": "2.0", "method":"get_live_cell","params": [{"hash": "0xbddb7c2559c2c3cdfc8f3cae2697ca75489521c352265cc9e60b4b2416ad5929", "index": 0}]}' -H 'content-type:application/json' 'http://localhost:8114'
```

```json
{
    "jsonrpc": "2.0",
    "result": {
        "cell": {
            "capacity": 50000,
            "type": null,
            "data": [],
            "lock": "0x321c1ca2887fb8eddaaa7e917399f71e63e03a1c83ff75ed12099a01115ea2ff"
        },
        "status": "current"
    },
    "id": 2
}
```

# get_tip_block_number

Returns the number of blocks in the longest blockchain.

## Examples

```shell
curl -d '{"id": 2, "jsonrpc": "2.0", "method":"get_tip_block_number","params": []}' -H 'content-type:application/json' 'http://localhost:8114'
```

```json
{
    "jsonrpc": "2.0",
    "result": 1240,
    "id": 2
}
```

# local_node_id

Returns the local node id.

## Examples

```shell
curl -d '{"id": 2, "jsonrpc": "2.0", "method":"local_node_id","params": []}' -H 'content-type:application/json' 'http://localhost:8114'
```

```json
{
    "jsonrpc": "2.0",
    "result": "/ip4/0.0.0.0/tcp/8115/p2p/QmdSxB6iTcbhj6gbZNthvJrwRkJrwnsohNpVixY4FtcZwv",
    "id": 2
}
```

# send_transaction

Creates new transaction.

## Parameters

Transaction - The transaction object.

    Version - Transaction version.
    Deps - Dependent cells.
    Inputs - Transaction inputs.
    Outputs - Transaction outputs.

## Examples

```shell
curl -d '{"id": 2, "jsonrpc": "2.0", "method":"send_transaction","params": [{"version":2, "deps":[], "inputs":[], "outputs":[]}]}' -H 'content-type:application/json' 'http://localhost:8114'
```

```json
{
    "jsonrpc": "2.0",
    "result": "0xd91110fe20b7137c884d5c515f591ceda89a177bf06c1a3eb99c8a970dda2cf5",
    "id": 2
}
```
