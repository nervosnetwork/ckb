# get_block

Returns the information about a block by hash.

## Parameters

    Hash - Hash of a block.

## Examples

```shell
curl -d '{"id": 2, "jsonrpc": "2.0", "method":"get_block","params": ["0x087c25e23e42f5d1e00e6984241b3711742d5e0eaf75d79a427276473e1de3f9"]}' -H 'content-type:application/json' 'http://localhost:8114'
```

```json
{
    "jsonrpc": "2.0",
    "result": {
        "commit_transactions": [
            {
                "deps": [],
                "hash": "0x3abd21e6e51674bb961bb4c5f3cee9faa5da30e64be10628dc1cef292cbae324",
                "inputs": [
                    {
                        "previous_output": {
                            "hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
                            "index": 4294967295
                        },
                        "unlock": {
                            "args": [],
                            "binary": "0x0100000000000000",
                            "reference": null,
                            "signed_args": [],
                            "version": 0
                        }
                    }
                ],
                "outputs": [
                    {
                        "capacity": 5000000,
                        "data": "0x",
                        "lock": "0x0da2fe99fe549e082d4ed483c2e968a89ea8d11aabf5d79e5cbf06522de6e674",
                        "type": null
                    }
                ],
                "version": 0
            }
        ],
        "header": {
            "cellbase_id": "0x3abd21e6e51674bb961bb4c5f3cee9faa5da30e64be10628dc1cef292cbae324",
            "difficulty": "0x100",
            "hash": "0x087c25e23e42f5d1e00e6984241b3711742d5e0eaf75d79a427276473e1de3f9",
            "number": 1,
            "parent_hash": "0x9b0bd5be9498a0b873d08e242fff306eec04fac7c59ce479b49ca92a8f649982",
            "seal": {
                "nonce": 16394887283531791882,
                "proof": "0xbd010000810200008a1300002e240000a9350000c4350000ea420000ca4d00005d5d0000766800004b6b000075730000"
            },
            "timestamp": 1545992487397,
            "txs_commit": "0x3abd21e6e51674bb961bb4c5f3cee9faa5da30e64be10628dc1cef292cbae324",
            "txs_proposal": "0x0000000000000000000000000000000000000000000000000000000000000000",
            "uncles_count": 0,
            "uncles_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
            "version": 0
        },
        "proposal_transactions": [],
        "uncles": []
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
curl -d '{"id": 2, "jsonrpc": "2.0", "method":"get_transaction","params": ["0x3abd21e6e51674bb961bb4c5f3cee9faa5da30e64be10628dc1cef292cbae324"]}' -H 'content-type:application/json' 'http://localhost:8114'
```

```json
{
    "jsonrpc": "2.0",
    "result": {
        "deps": [],
        "hash": "0x3abd21e6e51674bb961bb4c5f3cee9faa5da30e64be10628dc1cef292cbae324",
        "inputs": [
            {
                "previous_output": {
                    "hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
                    "index": 4294967295
                },
                "unlock": {
                    "args": [],
                    "binary": "0x0100000000000000",
                    "reference": null,
                    "signed_args": [],
                    "version": 0
                }
            }
        ],
        "outputs": [
            {
                "capacity": 5000000,
                "data": "0x",
                "lock": "0x0da2fe99fe549e082d4ed483c2e968a89ea8d11aabf5d79e5cbf06522de6e674",
                "type": null
            }
        ],
        "version": 0
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
        "cellbase_id": "0xa4ecd25e3b572dc078cf000bfa1d81f1b578eeb5245c166353682919d37ebf42",
        "difficulty": "0x100",
        "hash": "0x44483beaf890d4aac2b2df90a50d9236db4a810d08f0912c1981f4a1db8086fd",
        "number": 37,
        "parent_hash": "0x379e7f4e01c7264a27284571ff6c232229522fd462cb7ce2fd3d5252e3015d04",
        "seal": {
            "nonce": 2288736367820038381,
            "proof": "0x480a0000751200007f170000682f0000b1300000933d0000534a0000e34b0000f05c0000e5600000e87300005d750000"
        },
        "timestamp": 1545994242503,
        "txs_commit": "0xa4ecd25e3b572dc078cf000bfa1d81f1b578eeb5245c166353682919d37ebf42",
        "txs_proposal": "0x0000000000000000000000000000000000000000000000000000000000000000",
        "uncles_count": 0,
        "uncles_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
        "version": 0
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
curl -d '{"id": 2, "jsonrpc": "2.0", "method":"get_live_cell","params": [{"hash": "0x3abd21e6e51674bb961bb4c5f3cee9faa5da30e64be10628dc1cef292cbae324", "index": 0}]}' -H 'content-type:application/json' 'http://localhost:8114'
```

```json
{
    "jsonrpc": "2.0",
    "result": {
        "cell": {
            "capacity": 5000000,
            "data": "0x",
            "lock": "0x0da2fe99fe549e082d4ed483c2e968a89ea8d11aabf5d79e5cbf06522de6e674",
            "type": null
        },
        "status": "live"
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
