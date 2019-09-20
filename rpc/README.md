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
        "dao": "0xd040e08b93e8000039585fc1261dd700f6e18bdea63700000061eb7ada030000",
        "difficulty": "0x7a1200",
        "epoch": "0x7080018000001",
        "hash": "0x51c79aa53ac2326f6eb1c41690ac26a8505f770f95f93127b7ae4cadc12e598c",
        "nonce": "0x0",
        "number": "0x400",
        "parent_hash": "0xa1c400509437ba0d9a0a747c547e6b69f23398a6854e36ea144816ba4172bd74",
        "proposals_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
        "timestamp": "0x5cd2b117",
        "transactions_root": "0x389a19076c2dea3a81c7a93df5e0750e569b91a5e62930b0c3d1e58b1f292032",
        "uncles_count": "0x0",
        "uncles_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
        "version": "0x0",
        "witnesses_root": "0x45c5fe626dedcc5de6a6c30b7bd9efaa4e7f201f18f4cebb5603470264fa19d9"
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
    "result": "0x51c79aa53ac2326f6eb1c41690ac26a8505f770f95f93127b7ae4cadc12e598c"
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
<<<<<<< HEAD
        "0x51c79aa53ac2326f6eb1c41690ac26a8505f770f95f93127b7ae4cadc12e598c"
=======
<<<<<<< HEAD
<<<<<<< HEAD
        "0x575122d7ef96d62af241ec96af1ea20f4f3a542d8995cf510aac5c5adddba451"
=======
<<<<<<< HEAD
        "0xc73a331428dd9ef69b8073c248bfae9dc7c27942bb1cb70581e880bd3020d7da"
=======
        "0x4530dab1fbeca428c900201ae1a925ffe2437d227bfca52e12635c274aa579ee"
>>>>>>> refactor: change args from `Vec<Bytes>` to `Bytes`
>>>>>>> refactor: change args from `Vec<Bytes>` to `Bytes`
=======
        "0x7275b6d941cb0cb99e5d39b560b6f6ad9e1bb945fe493f226dae3ab18ba3a034"
>>>>>>> chore: rebase with develop branch
>>>>>>> chore: rebase with develop branch
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
            "dao": "0xd040e08b93e8000039585fc1261dd700f6e18bdea63700000061eb7ada030000",
            "difficulty": "0x7a1200",
            "epoch": "0x7080018000001",
<<<<<<< HEAD
            "hash": "0x51c79aa53ac2326f6eb1c41690ac26a8505f770f95f93127b7ae4cadc12e598c",
=======
<<<<<<< HEAD
            "hash": "0x575122d7ef96d62af241ec96af1ea20f4f3a542d8995cf510aac5c5adddba451",
>>>>>>> chore: rebase with develop branch
            "nonce": "0x0",
            "number": "0x400",
            "parent_hash": "0xa1c400509437ba0d9a0a747c547e6b69f23398a6854e36ea144816ba4172bd74",
            "proposals_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
            "timestamp": "0x5cd2b117",
            "transactions_root": "0x389a19076c2dea3a81c7a93df5e0750e569b91a5e62930b0c3d1e58b1f292032",
            "uncles_count": "0x0",
            "uncles_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
            "version": "0x0",
<<<<<<< HEAD
            "witnesses_root": "0x45c5fe626dedcc5de6a6c30b7bd9efaa4e7f201f18f4cebb5603470264fa19d9"
=======
            "witnesses_root": "0xa202ae700692d18d5b9944faa1021edf6c2551fd5e46df6d427d7a1a1018e438"
=======
            "hash": "0x7275b6d941cb0cb99e5d39b560b6f6ad9e1bb945fe493f226dae3ab18ba3a034",
            "nonce": "0x0",
            "number": "0x400",
            "parent_hash": "0x6087b0e6983e0c1278d9224a0cf0b1dd0ed68ea74ecf5c4a92fd22811b248a43",
            "proposals_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
            "timestamp": "0x5cd2b117",
            "transactions_root": "0x546c99494650bdf50e18690a1d2b874c58f9f8fa3725e10414c5cae7931e3dcd",
            "uncles_count": "0x0",
            "uncles_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
            "version": "0x0",
            "witnesses_root": "0xa1c70211d16c4a013723bc37fedb1e9786d62ccd9bf193705a747ca4b3689b6d"
>>>>>>> chore: rebase with develop branch
>>>>>>> chore: rebase with develop branch
        },
        "proposals": [],
        "transactions": [
            {
                "cell_deps": [],
<<<<<<< HEAD
                "hash": "0x389a19076c2dea3a81c7a93df5e0750e569b91a5e62930b0c3d1e58b1f292032",
=======
<<<<<<< HEAD
                "hash": "0x1fff3c593627641d06e83cc20a9abfd78a8dbd9e8c02d50a2e8b3e395f883cfe",
=======
                "hash": "0x546c99494650bdf50e18690a1d2b874c58f9f8fa3725e10414c5cae7931e3dcd",
>>>>>>> chore: rebase with develop branch
>>>>>>> chore: rebase with develop branch
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
                        "capacity": "0x104ca73381",
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
                    "0x3500000010000000300000003100000028e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a50000000000"
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
<<<<<<< HEAD
        "0x51c79aa53ac2326f6eb1c41690ac26a8505f770f95f93127b7ae4cadc12e598c"
=======
<<<<<<< HEAD
<<<<<<< HEAD
        "0x575122d7ef96d62af241ec96af1ea20f4f3a542d8995cf510aac5c5adddba451"
=======
<<<<<<< HEAD
        "0xc73a331428dd9ef69b8073c248bfae9dc7c27942bb1cb70581e880bd3020d7da"
=======
        "0x4530dab1fbeca428c900201ae1a925ffe2437d227bfca52e12635c274aa579ee"
>>>>>>> refactor: change args from `Vec<Bytes>` to `Bytes`
>>>>>>> refactor: change args from `Vec<Bytes>` to `Bytes`
=======
        "0x7275b6d941cb0cb99e5d39b560b6f6ad9e1bb945fe493f226dae3ab18ba3a034"
>>>>>>> chore: rebase with develop branch
>>>>>>> chore: rebase with develop branch
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
        "dao": "0xd040e08b93e8000039585fc1261dd700f6e18bdea63700000061eb7ada030000",
        "difficulty": "0x7a1200",
        "epoch": "0x7080018000001",
<<<<<<< HEAD
        "hash": "0x51c79aa53ac2326f6eb1c41690ac26a8505f770f95f93127b7ae4cadc12e598c",
=======
<<<<<<< HEAD
        "hash": "0x575122d7ef96d62af241ec96af1ea20f4f3a542d8995cf510aac5c5adddba451",
>>>>>>> chore: rebase with develop branch
        "nonce": "0x0",
        "number": "0x400",
        "parent_hash": "0xa1c400509437ba0d9a0a747c547e6b69f23398a6854e36ea144816ba4172bd74",
        "proposals_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
        "timestamp": "0x5cd2b117",
        "transactions_root": "0x389a19076c2dea3a81c7a93df5e0750e569b91a5e62930b0c3d1e58b1f292032",
        "uncles_count": "0x0",
        "uncles_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
        "version": "0x0",
<<<<<<< HEAD
        "witnesses_root": "0x45c5fe626dedcc5de6a6c30b7bd9efaa4e7f201f18f4cebb5603470264fa19d9"
=======
        "witnesses_root": "0xa202ae700692d18d5b9944faa1021edf6c2551fd5e46df6d427d7a1a1018e438"
=======
        "hash": "0x7275b6d941cb0cb99e5d39b560b6f6ad9e1bb945fe493f226dae3ab18ba3a034",
        "nonce": "0x0",
        "number": "0x400",
        "parent_hash": "0x6087b0e6983e0c1278d9224a0cf0b1dd0ed68ea74ecf5c4a92fd22811b248a43",
        "proposals_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
        "timestamp": "0x5cd2b117",
        "transactions_root": "0x546c99494650bdf50e18690a1d2b874c58f9f8fa3725e10414c5cae7931e3dcd",
        "uncles_count": "0x0",
        "uncles_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
        "version": "0x0",
        "witnesses_root": "0xa1c70211d16c4a013723bc37fedb1e9786d62ccd9bf193705a747ca4b3689b6d"
>>>>>>> chore: rebase with develop branch
>>>>>>> chore: rebase with develop branch
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
        "dao": "0xd040e08b93e8000039585fc1261dd700f6e18bdea63700000061eb7ada030000",
        "difficulty": "0x7a1200",
        "epoch": "0x7080018000001",
<<<<<<< HEAD
        "hash": "0x51c79aa53ac2326f6eb1c41690ac26a8505f770f95f93127b7ae4cadc12e598c",
=======
<<<<<<< HEAD
        "hash": "0x575122d7ef96d62af241ec96af1ea20f4f3a542d8995cf510aac5c5adddba451",
>>>>>>> chore: rebase with develop branch
        "nonce": "0x0",
        "number": "0x400",
        "parent_hash": "0xa1c400509437ba0d9a0a747c547e6b69f23398a6854e36ea144816ba4172bd74",
        "proposals_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
        "timestamp": "0x5cd2b117",
        "transactions_root": "0x389a19076c2dea3a81c7a93df5e0750e569b91a5e62930b0c3d1e58b1f292032",
        "uncles_count": "0x0",
        "uncles_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
        "version": "0x0",
<<<<<<< HEAD
        "witnesses_root": "0x45c5fe626dedcc5de6a6c30b7bd9efaa4e7f201f18f4cebb5603470264fa19d9"
=======
        "witnesses_root": "0xa202ae700692d18d5b9944faa1021edf6c2551fd5e46df6d427d7a1a1018e438"
=======
        "hash": "0x7275b6d941cb0cb99e5d39b560b6f6ad9e1bb945fe493f226dae3ab18ba3a034",
        "nonce": "0x0",
        "number": "0x400",
        "parent_hash": "0x6087b0e6983e0c1278d9224a0cf0b1dd0ed68ea74ecf5c4a92fd22811b248a43",
        "proposals_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
        "timestamp": "0x5cd2b117",
        "transactions_root": "0x546c99494650bdf50e18690a1d2b874c58f9f8fa3725e10414c5cae7931e3dcd",
        "uncles_count": "0x0",
        "uncles_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
        "version": "0x0",
        "witnesses_root": "0xa1c70211d16c4a013723bc37fedb1e9786d62ccd9bf193705a747ca4b3689b6d"
>>>>>>> chore: rebase with develop branch
>>>>>>> chore: rebase with develop branch
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
<<<<<<< HEAD
            "block_hash": "0x490af0885bc480bf38235526ad4573cd02cf77ebbe0655570e851cd72c2e6a6a",
            "capacity": "0x2ca7071b9e",
=======
<<<<<<< HEAD
            "block_hash": "0xcc22c9d0bcfeeaff1253dbb98ba35457c4ce736df53cc9ca19aa5735d6b6f0e4",
=======
            "block_hash": "0x9fbe76384d0d112a932ff24b5d5ab63a34c526f08be0d415adfb0fe014f67842",
>>>>>>> chore: rebase with develop branch
            "capacity": "0x1d1a94a200",
>>>>>>> chore: rebase with develop branch
            "lock": {
                "args": "0x",
                "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
                "hash_type": "data"
            },
            "out_point": {
                "index": "0x0",
<<<<<<< HEAD
                "tx_hash": "0x6926a77905699715aea5a2ddbf03cb86b1b4d7939b2be3a555db7949452a0aa3"
            }
        },
        {
            "block_hash": "0x10b4df711a951142982a74c329fa57b63387cd73cff76372ad5927df9ecdc602",
            "capacity": "0x2ca7071b9e",
=======
                "tx_hash": "0xf5aaface17d42b00d932615921c63ee1bb12a5ae72cba45a9c28bdf7db88e24f"
            }
        },
        {
<<<<<<< HEAD
            "block_hash": "0x0678df4ff71385b8ac2965f4653de69a6c475351d6e6423f93e7ec5122f28c01",
=======
            "block_hash": "0x877c30d8413d21d72c6b175bcb391a793dbd9e496bbb3106457dbca342f7fd62",
>>>>>>> chore: rebase with develop branch
            "capacity": "0x1d1a94a200",
>>>>>>> chore: rebase with develop branch
            "lock": {
                "args": "0x",
                "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
                "hash_type": "data"
            },
            "out_point": {
                "index": "0x0",
<<<<<<< HEAD
                "tx_hash": "0x3f0aa88e203ff3f13a06b9153946dfc5b163aad14cc9847a2bee8e5e08acb46b"
=======
                "tx_hash": "0xf1aa3608559b616693d64f14584bd4cc392da36f1e8f001fe9c6a1ad215ce6a6"
>>>>>>> chore: rebase with develop branch
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
<<<<<<< HEAD
        "0xbdd5df6248d5cca6f4652953d4f85b3ca65219d966a9d0a761d9ff764df92e83"
=======
<<<<<<< HEAD
<<<<<<< HEAD
        "0xd7572fb4c1bf2acd069b6c574cc3d69464151e97fbd746aa1b62942ae6fd7c84"
=======
<<<<<<< HEAD
        "0xba86cc2cb21832bf4a84c032eb6e8dc422385cc8f8efb84eb0bc5fe0b0b9aece"
=======
        "0xdb9835667dfd0ed61eb6d84d5dd2da71d32b5e0eee2b236f82f93ee2ebfbc6a4"
>>>>>>> refactor: change args from `Vec<Bytes>` to `Bytes`
>>>>>>> refactor: change args from `Vec<Bytes>` to `Bytes`
=======
        "0xe6d0aa043922568e3e6c0972252d5e30d0f2c36d61178317e090cb735b6d2a52"
>>>>>>> chore: rebase with develop branch
>>>>>>> chore: rebase with develop branch
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
<<<<<<< HEAD
            "hash": "0xbdd5df6248d5cca6f4652953d4f85b3ca65219d966a9d0a761d9ff764df92e83",
=======
<<<<<<< HEAD
            "hash": "0xd7572fb4c1bf2acd069b6c574cc3d69464151e97fbd746aa1b62942ae6fd7c84",
>>>>>>> chore: rebase with develop branch
            "header_deps": [
                "0xb7d114888ba196ee445728950b6d26a4e0fbad4c9c86e6558e595b7a4489fa37"
=======
            "hash": "0xe6d0aa043922568e3e6c0972252d5e30d0f2c36d61178317e090cb735b6d2a52",
            "header_deps": [
                "0xc5307f1ca86b12221ba9cafd783261d9562e8c5369acb2f1873e457c966ed279"
>>>>>>> chore: rebase with develop branch
            ],
            "inputs": [
                {
                    "previous_output": {
                        "index": "0x0",
<<<<<<< HEAD
                        "tx_hash": "0x6926a77905699715aea5a2ddbf03cb86b1b4d7939b2be3a555db7949452a0aa3"
=======
                        "tx_hash": "0xf5aaface17d42b00d932615921c63ee1bb12a5ae72cba45a9c28bdf7db88e24f"
>>>>>>> chore: rebase with develop branch
                    },
                    "since": "0x0"
                }
            ],
            "outputs": [
                {
                    "capacity": "0x174876e800",
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
<<<<<<< HEAD
        "0x51c79aa53ac2326f6eb1c41690ac26a8505f770f95f93127b7ae4cadc12e598c"
=======
<<<<<<< HEAD
<<<<<<< HEAD
        "0x575122d7ef96d62af241ec96af1ea20f4f3a542d8995cf510aac5c5adddba451"
=======
<<<<<<< HEAD
        "0xc73a331428dd9ef69b8073c248bfae9dc7c27942bb1cb70581e880bd3020d7da"
=======
        "0x4530dab1fbeca428c900201ae1a925ffe2437d227bfca52e12635c274aa579ee"
>>>>>>> refactor: change args from `Vec<Bytes>` to `Bytes`
>>>>>>> refactor: change args from `Vec<Bytes>` to `Bytes`
=======
        "0x7275b6d941cb0cb99e5d39b560b6f6ad9e1bb945fe493f226dae3ab18ba3a034"
>>>>>>> chore: rebase with develop branch
>>>>>>> chore: rebase with develop branch
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
        "secondary": "0x21711264",
        "total": "0x104ca73381",
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
            "dao": "0xd040e08b93e8000039585fc1261dd700f6e18bdea63700000061eb7ada030000",
            "difficulty": "0x7a1200",
            "epoch": "0x7080018000001",
<<<<<<< HEAD
            "hash": "0x51c79aa53ac2326f6eb1c41690ac26a8505f770f95f93127b7ae4cadc12e598c",
=======
<<<<<<< HEAD
            "hash": "0x575122d7ef96d62af241ec96af1ea20f4f3a542d8995cf510aac5c5adddba451",
>>>>>>> chore: rebase with develop branch
            "nonce": "0x0",
            "number": "0x400",
            "parent_hash": "0xa1c400509437ba0d9a0a747c547e6b69f23398a6854e36ea144816ba4172bd74",
            "proposals_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
            "timestamp": "0x5cd2b117",
            "transactions_root": "0x389a19076c2dea3a81c7a93df5e0750e569b91a5e62930b0c3d1e58b1f292032",
            "uncles_count": "0x0",
            "uncles_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
            "version": "0x0",
<<<<<<< HEAD
            "witnesses_root": "0x45c5fe626dedcc5de6a6c30b7bd9efaa4e7f201f18f4cebb5603470264fa19d9"
=======
            "witnesses_root": "0xa202ae700692d18d5b9944faa1021edf6c2551fd5e46df6d427d7a1a1018e438"
=======
            "hash": "0x7275b6d941cb0cb99e5d39b560b6f6ad9e1bb945fe493f226dae3ab18ba3a034",
            "nonce": "0x0",
            "number": "0x400",
            "parent_hash": "0x6087b0e6983e0c1278d9224a0cf0b1dd0ed68ea74ecf5c4a92fd22811b248a43",
            "proposals_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
            "timestamp": "0x5cd2b117",
            "transactions_root": "0x546c99494650bdf50e18690a1d2b874c58f9f8fa3725e10414c5cae7931e3dcd",
            "uncles_count": "0x0",
            "uncles_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
            "version": "0x0",
            "witnesses_root": "0xa1c70211d16c4a013723bc37fedb1e9786d62ccd9bf193705a747ca4b3689b6d"
>>>>>>> chore: rebase with develop branch
>>>>>>> chore: rebase with develop branch
        },
        "proposals": [],
        "transactions": [
            {
                "cell_deps": [],
<<<<<<< HEAD
                "hash": "0x389a19076c2dea3a81c7a93df5e0750e569b91a5e62930b0c3d1e58b1f292032",
=======
<<<<<<< HEAD
                "hash": "0x1fff3c593627641d06e83cc20a9abfd78a8dbd9e8c02d50a2e8b3e395f883cfe",
=======
                "hash": "0x546c99494650bdf50e18690a1d2b874c58f9f8fa3725e10414c5cae7931e3dcd",
>>>>>>> chore: rebase with develop branch
>>>>>>> chore: rebase with develop branch
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
                        "capacity": "0x104ca73381",
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
                    "0x3500000010000000300000003100000028e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a50000000000"
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
<<<<<<< HEAD
                "0xb7d114888ba196ee445728950b6d26a4e0fbad4c9c86e6558e595b7a4489fa37"
=======
<<<<<<< HEAD
<<<<<<< HEAD
                "0xb7d114888ba196ee445728950b6d26a4e0fbad4c9c86e6558e595b7a4489fa37"
=======
<<<<<<< HEAD
                "0x8033e126475d197f2366bbc2f30b907d15af85c9d9533253c6f0787dcbbb509e"
=======
                "0x62b92fe2550cfa6b8c40e44c632d40229d7d5a4659c33b15829159279dfb73e3"
>>>>>>> refactor: change args from `Vec<Bytes>` to `Bytes`
>>>>>>> refactor: change args from `Vec<Bytes>` to `Bytes`
=======
                "0xc5307f1ca86b12221ba9cafd783261d9562e8c5369acb2f1873e457c966ed279"
>>>>>>> chore: rebase with develop branch
>>>>>>> chore: rebase with develop branch
            ],
            "inputs": [
                {
                    "previous_output": {
                        "index": "0x0",
                        "tx_hash": "0x6926a77905699715aea5a2ddbf03cb86b1b4d7939b2be3a555db7949452a0aa3"
                    },
                    "since": "0x0"
                }
            ],
            "outputs": [
                {
                    "capacity": "0x174876e800",
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
<<<<<<< HEAD
                "0xb7d114888ba196ee445728950b6d26a4e0fbad4c9c86e6558e595b7a4489fa37"
=======
<<<<<<< HEAD
<<<<<<< HEAD
                "0xb7d114888ba196ee445728950b6d26a4e0fbad4c9c86e6558e595b7a4489fa37"
=======
<<<<<<< HEAD
                "0x8033e126475d197f2366bbc2f30b907d15af85c9d9533253c6f0787dcbbb509e"
=======
                "0x62b92fe2550cfa6b8c40e44c632d40229d7d5a4659c33b15829159279dfb73e3"
>>>>>>> refactor: change args from `Vec<Bytes>` to `Bytes`
>>>>>>> refactor: change args from `Vec<Bytes>` to `Bytes`
=======
                "0xc5307f1ca86b12221ba9cafd783261d9562e8c5369acb2f1873e457c966ed279"
>>>>>>> chore: rebase with develop branch
>>>>>>> chore: rebase with develop branch
            ],
            "inputs": [
                {
                    "previous_output": {
                        "index": "0x0",
                        "tx_hash": "0x6926a77905699715aea5a2ddbf03cb86b1b4d7939b2be3a555db7949452a0aa3"
                    },
                    "since": "0x0"
                }
            ],
            "outputs": [
                {
                    "capacity": "0x174876e800",
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
<<<<<<< HEAD
    "result": "0xbdd5df6248d5cca6f4652953d4f85b3ca65219d966a9d0a761d9ff764df92e83"
=======
<<<<<<< HEAD
    "result": "0xd7572fb4c1bf2acd069b6c574cc3d69464151e97fbd746aa1b62942ae6fd7c84"
=======
    "result": "0xe6d0aa043922568e3e6c0972252d5e30d0f2c36d61178317e090cb735b6d2a52"
>>>>>>> chore: rebase with develop branch
>>>>>>> chore: rebase with develop branch
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
<<<<<<< HEAD
        "0x51c79aa53ac2326f6eb1c41690ac26a8505f770f95f93127b7ae4cadc12e598c"
=======
<<<<<<< HEAD
<<<<<<< HEAD
        "0x575122d7ef96d62af241ec96af1ea20f4f3a542d8995cf510aac5c5adddba451"
=======
<<<<<<< HEAD
        "0xc73a331428dd9ef69b8073c248bfae9dc7c27942bb1cb70581e880bd3020d7da"
=======
        "0x4530dab1fbeca428c900201ae1a925ffe2437d227bfca52e12635c274aa579ee"
>>>>>>> refactor: change args from `Vec<Bytes>` to `Bytes`
>>>>>>> refactor: change args from `Vec<Bytes>` to `Bytes`
=======
        "0x7275b6d941cb0cb99e5d39b560b6f6ad9e1bb945fe493f226dae3ab18ba3a034"
>>>>>>> chore: rebase with develop branch
>>>>>>> chore: rebase with develop branch
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
<<<<<<< HEAD
        "block_hash": "0x51c79aa53ac2326f6eb1c41690ac26a8505f770f95f93127b7ae4cadc12e598c",
=======
<<<<<<< HEAD
        "block_hash": "0x575122d7ef96d62af241ec96af1ea20f4f3a542d8995cf510aac5c5adddba451",
=======
        "block_hash": "0x7275b6d941cb0cb99e5d39b560b6f6ad9e1bb945fe493f226dae3ab18ba3a034",
>>>>>>> chore: rebase with develop branch
>>>>>>> chore: rebase with develop branch
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
<<<<<<< HEAD
            "block_hash": "0x51c79aa53ac2326f6eb1c41690ac26a8505f770f95f93127b7ae4cadc12e598c",
=======
<<<<<<< HEAD
            "block_hash": "0x575122d7ef96d62af241ec96af1ea20f4f3a542d8995cf510aac5c5adddba451",
=======
            "block_hash": "0x7275b6d941cb0cb99e5d39b560b6f6ad9e1bb945fe493f226dae3ab18ba3a034",
>>>>>>> chore: rebase with develop branch
>>>>>>> chore: rebase with develop branch
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
                "capacity": "0x2ca7071b9e",
                "lock": {
                    "args": "0x",
                    "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
                    "hash_type": "data"
                },
                "type": null
            },
            "created_by": {
                "block_number": "0x1",
                "index": "0x0",
<<<<<<< HEAD
                "tx_hash": "0x6926a77905699715aea5a2ddbf03cb86b1b4d7939b2be3a555db7949452a0aa3"
=======
                "tx_hash": "0xf5aaface17d42b00d932615921c63ee1bb12a5ae72cba45a9c28bdf7db88e24f"
>>>>>>> chore: rebase with develop branch
            }
        },
        {
            "cell_output": {
                "capacity": "0x2ca7071b9e",
                "lock": {
                    "args": "0x",
                    "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
                    "hash_type": "data"
                },
                "type": null
            },
            "created_by": {
                "block_number": "0x2",
                "index": "0x0",
<<<<<<< HEAD
                "tx_hash": "0x3f0aa88e203ff3f13a06b9153946dfc5b163aad14cc9847a2bee8e5e08acb46b"
=======
                "tx_hash": "0xf1aa3608559b616693d64f14584bd4cc392da36f1e8f001fe9c6a1ad215ce6a6"
>>>>>>> chore: rebase with develop branch
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
<<<<<<< HEAD
                "tx_hash": "0x6926a77905699715aea5a2ddbf03cb86b1b4d7939b2be3a555db7949452a0aa3"
=======
                "tx_hash": "0xf5aaface17d42b00d932615921c63ee1bb12a5ae72cba45a9c28bdf7db88e24f"
>>>>>>> chore: rebase with develop branch
            }
        },
        {
            "consumed_by": null,
            "created_by": {
                "block_number": "0x2",
                "index": "0x0",
<<<<<<< HEAD
                "tx_hash": "0x3f0aa88e203ff3f13a06b9153946dfc5b163aad14cc9847a2bee8e5e08acb46b"
=======
                "tx_hash": "0xf1aa3608559b616693d64f14584bd4cc392da36f1e8f001fe9c6a1ad215ce6a6"
>>>>>>> chore: rebase with develop branch
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
                "dao": "0xd040e08b93e8000039585fc1261dd700f6e18bdea63700000061eb7ada030000",
                "difficulty": "0x7a1200",
                "epoch": "0x7080018000001",
                "nonce": "0x0",
                "number": "0x400",
                "parent_hash": "0xa1c400509437ba0d9a0a747c547e6b69f23398a6854e36ea144816ba4172bd74",
                "proposals_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
                "timestamp": "0x5cd2b117",
                "transactions_root": "0x389a19076c2dea3a81c7a93df5e0750e569b91a5e62930b0c3d1e58b1f292032",
                "uncles_count": "0x0",
                "uncles_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
                "version": "0x0",
                "witnesses_root": "0x45c5fe626dedcc5de6a6c30b7bd9efaa4e7f201f18f4cebb5603470264fa19d9"
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
                            "capacity": "0x104ca73381",
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
    "result": "0x51c79aa53ac2326f6eb1c41690ac26a8505f770f95f93127b7ae4cadc12e598c"
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
<<<<<<< HEAD
                "0xb7d114888ba196ee445728950b6d26a4e0fbad4c9c86e6558e595b7a4489fa37"
=======
<<<<<<< HEAD
<<<<<<< HEAD
                "0xb7d114888ba196ee445728950b6d26a4e0fbad4c9c86e6558e595b7a4489fa37"
=======
<<<<<<< HEAD
                "0x8033e126475d197f2366bbc2f30b907d15af85c9d9533253c6f0787dcbbb509e"
=======
                "0x62b92fe2550cfa6b8c40e44c632d40229d7d5a4659c33b15829159279dfb73e3"
>>>>>>> refactor: change args from `Vec<Bytes>` to `Bytes`
>>>>>>> refactor: change args from `Vec<Bytes>` to `Bytes`
=======
                "0xc5307f1ca86b12221ba9cafd783261d9562e8c5369acb2f1873e457c966ed279"
>>>>>>> chore: rebase with develop branch
>>>>>>> chore: rebase with develop branch
            ],
            "inputs": [
                {
                    "previous_output": {
                        "index": "0x0",
                        "tx_hash": "0x6926a77905699715aea5a2ddbf03cb86b1b4d7939b2be3a555db7949452a0aa3"
                    },
                    "since": "0x0"
                }
            ],
            "outputs": [
                {
                    "capacity": "0x174876e800",
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
<<<<<<< HEAD
    "result": "0xbdd5df6248d5cca6f4652953d4f85b3ca65219d966a9d0a761d9ff764df92e83"
=======
<<<<<<< HEAD
    "result": "0xd7572fb4c1bf2acd069b6c574cc3d69464151e97fbd746aa1b62942ae6fd7c84"
=======
    "result": "0xe6d0aa043922568e3e6c0972252d5e30d0f2c36d61178317e090cb735b6d2a52"
>>>>>>> chore: rebase with develop branch
>>>>>>> chore: rebase with develop branch
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
