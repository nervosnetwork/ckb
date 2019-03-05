# Data Structures of Cell/Script/Transaction/Block

This documents list all the basic structures one may need to know in order to develop on CKB, or mine CKB coins. 

* [Cell](#Cell)
* [Script](#Script)
* [Transaction](#Transaction)
* [Block](#Block)



## Cell

### Example

```json
{
    "capacity": 5000000,
    "data": "0x",
    "lock": "0xa58a960b28d6e283546e38740e80142da94f88e88d5114d8dc91312b8da4765a",
    "type": null
}
```

## Description

| Name       | Type       | Description                                                  |
| :--------- | :--------- | :----------------------------------------------------------- |
| `capacity` | uint64     | **The size of the cell.** When a new cell is generated (via transaction), one of the verification rule is `capacity = len(capacity)+len(data)+len(type)+len(lock)`. This value also represents the balance of CKB coin, just like the `balance` field in the Bitcoin's UTXO. (E.g. Alice owns 100 CKB coins means she can unlock a group of cells that has 100 amount of `capacity` in total.) |
| `data`     | Bytes      | **Arbitrary data.** This part is for storing states or scripts.  In order to make this cell valid on-chain, the data filled in this field should comply with the logics and rules defined by `type` or `lock`. |
| `type`     | `Script`   | **A Script that defines the type of the cell.** In a transaction, if an input cell and an output cell has the same `type` field, then the `data` part of these two cells is limited by the `type` script upon the transaction verification. (I.e. `type` is a script that limits how the `data` field of the new cells can be changed from the old cells.) `type` is required to has a data structure of `script`. This field can be empty in a Cell. |
| `lock`     | H256(hash) | **The hash of a Script that defines the ownership of the cell**, just like the `lock` field in the Bitcoin's UTXO. Whoever can provide an unlock script that has the same hash of a cell's `lock` hash can use this cell as input in an transaction (i.e. has the ownership of this cell). This is similar to the P2SH scheme in Bitcoin. |



More information about Cell can be found in the [whitepaper](https://github.com/nervosnetwork/rfcs/blob/afe50463bb620393b179bd8f08c263b78e366ab3/rfcs/0002-ckb/0002-ckb.md#42-cell).



## Script

### Example

```json
{
  "version": 0,
  "reference": "0x12b464bcab8f55822501cdb91ea35ea707d72ec970363972388a0c49b94d377c",
  "signed_args": [
    "024a501efd328e062c8675f2365970728c859c592beeefd6be8ead3d901330bc01"
  ],
  "args": [
    "3044022038f282cffdd26e2a050d7779ddc29be81a7e2f8a73706d2b7a6fde8a78e950ee0220538657b4c01be3e77827a82e92d33a923e864c55b88fd18cd5e5b25597432e9b",
    "1"
  ]
}
```



### Description

| Name          | Type    | Description                                                  |
| :------------ | :------ | :----------------------------------------------------------- |
| `version`     | uint8   | **The version of the script.** It‘s used to distinguish transactions when there's a fork happened to the blockchain system. |
| `binary`      | Bytes   | **ELF formatted binary that contains an RISC-V based script.** This part of data is loaded into an CKB-VM instance when they are specified upon the transaction verification. |
| `reference`   | Bytes   | **The hash of the script that is referred by this script.** It is possible to refer the script in another cell on-chain as the binary code in this script, instead of entering the binary directly into the script. **Notice:** This is part only works when the `binary` field is empty. |
| `args`        | [Bytes] | **An array of arguments for the script.** The arguments here are imported into the CKB-VM instance as input arguments for the scripts. This part is NOT used when calculating the hash of the script. |
| `signed_args` | [Bytes] | **An array of signed arguments for the script.** The arguments with signatures. The `signed_args` and the `args` will be connected into a single vector, and imported into the CKB-VM instance as input arguments. |



More information about Script can be [here](https://github.com/nervosnetwork/ckb-demo-ruby-sdk/blob/27669cd6b4f56f8977b725fc0e6582b288ab2b82/docs/how-to-write-contracts.md#script-model).



## Transaction

### Example

```json
{
    "deps": [],
    "hash": "0x4707810253259258e3091934dc5b543403c10cc899859e077fe26067f8d52dc0",
    "inputs": [
      {
        "previous_output": {
          "hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
          "index": 4294967295
        },
        "unlock": {
          "args": [],
          "binary": "0x0d00000000000000",
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
        "lock": "0xa58a960b28d6e283546e38740e80142da94f88e88d5114d8dc91312b8da4765a",
        "type": null
      }
    ],
    "version": 0
}
```

### Description

| Name                     | Type         | Description                                                  |
| ------------------------ | ------------ | ------------------------------------------------------------ |
| `version`                | uint32       | **The version of the transaction.** It‘s used to distinguish transactions when there's a fork happened to the blockchain system. |
| `deps`                   | [`cell`]     | **An array of cells that are dependencies of this transaction.** Only live cells can be listed here. The cells listed are read-only. |
| `inputs.previous_output` | [`outpoint`] | **An array of cell outpoints that point to the cells used as inputs.** Input cells are in fact the output of previous transactions, hence they are noted as `previous_output` here. These cells are referred through  `outpoint`, which contains the transaction `hash` of the previous transaction, as well as this cell's `index` in its transaction's output list. |
| `inputs.unlock`          | [`script`]   | **An array of scripts for unlocking their related input cells** (i.e. `previous_output`). See [here](https://github.com/nervosnetwork/ckb-demo-ruby-sdk/blob/develop/docs/how-to-write-contracts.md) for how to program this part. |
| `outputs`                | [`cell`]     | **An array of cells that are used as outputs**, i.e. the newly generated cells. These are the cells may be used as inputs for other transactions. |



More information about the Transaction of Nervos CKB can be found in [whitepaper](https://github.com/nervosnetwork/rfcs/blob/afe50463bb620393b179bd8f08c263b78e366ab3/rfcs/0002-ckb/0002-ckb.md#44-transaction).



## Block

### Example

```json
{
"commit_transactions": [
    {
    "deps": [],
    "hash": "0xabeb06aea75b59ec316db9d21243ee3f0b0ad0723e50f57761cef7e07974b9b5",
    "inputs": [
        {
        "previous_output": {
            "hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
            "index": 4294967295
        },
        "unlock": {
            "args": [],
            "binary": "0x0b00000000000000",
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
        "lock": "0xa58a960b28d6e283546e38740e80142da94f88e88d5114d8dc91312b8da4765a",
        "type": null
        }
    ],
    "version": 0
    }
],
"header": {
    "cellbase_id": "0xabeb06aea75b59ec316db9d21243ee3f0b0ad0723e50f57761cef7e07974b9b5",
    "difficulty": "0x100",
    "hash": "0xcddd882eff5edd2f7db25074cbbdc1d21cd698f60d6fb39412ef91d19eb900e8",
    "number": 11,
    "parent_hash": "0x255f65bf9dc00bcd9f9b8be8624be222cba16b51366208a8267f1925eb40e7e4",
    "seal": {
        "nonce": 503529102265201399,
        "proof": "0x"
    },
    "timestamp": 1551155125985,
    "txs_commit": "0xabeb06aea75b59ec316db9d21243ee3f0b0ad0723e50f57761cef7e07974b9b5",
    "txs_proposal": "0x0000000000000000000000000000000000000000000000000000000000000000",
    "uncles_count": 1,
    "uncles_hash": "0x99cf8710e59303bfac236b57256fcea2c58192f2c9c39d1ea4c19cbcf88b4952",
    "version": 0
},
"proposal_transactions": [],
"uncles": [
    {
    "cellbase": {
        ...
    },
    "header": {
        ...
    },
    "proposal_transactions": []
    }
]
}
```

### Description

#### Block

| Name                    | Type            | Description                                                  |
| ----------------------- | --------------- | ------------------------------------------------------------ |
| `header`                | `Header`        | **The block header of the block.** This part contains some metadata of the block. |
| `commit_trasactions`    | [`Transaction`] | **An array of transactions contained in the block.** This is where the miner put the received transactions. |
| `proposal_transactions` | [string]        | **An array of hex-encoded short transaction ID.**            |
| `uncles`                | [`UncleBlock`]  | **An array of uncle blocks of the block.**                   |

#### Header

| Name           | Type                | Description                                                  |
| -------------- | ------------------- | ------------------------------------------------------------ |
| `cellbase_id`  | H256(hash)          | **The hash of the Cellbase transaction.** Cellbase transaction is just like the coinable transaction in Bitcoin. It's the transaction added by the miner who mined this block, by which the miner receives block reward for successfully mined the block. |
| `difficulty`   | Bytes               | **The difficulty of the PoW puzzle.**                        |
| `hash`         | H256(hash)          | **The block hash.**                                          |
| `number`       | uint64              | **The block height.**                                        |
| `parent_hash`  | H256(hash)          | **The hash of the parent block.**                            |
| `seal`         | `nonce` and `proof` | **The seal of a block.** After finished the block assembling, the miner can start to do the calculation for finding the solution of the PoW puzzle. The "solution" here is called `seal`. |
| `seal.nonce`   | uint64              | **The nonce.** Similar to [the nonce in Bitcoin](https://en.bitcoin.it/wiki/Nonce). |
| `seal.proof`   | Bytes               | **The solution of the PoW puzzle.**                          |
| `timestamp`    | uint64              | **A [Unix time](http://en.wikipedia.org/wiki/Unix_time) timestamp.** |
| `txs_commit`   | H256(hash)          | **The Merkle Root of the Merkle trie with the hash of transactions as leaves.** |
| `txs_proposal` | H256(hash)          | **The Merkle Root of the Merkle trie with the hash of short transaction IDs as leaves.** |
| `uncles_count` | uint32              | **The number of uncle blocks.**                              |
| `uncles_hash`  | H256(hash)          | **The Merkle Root of the Merkle trie with the hash of uncle blocks as leaves.** |
| `version`      | uint32              | **The version of the block**. This is for solving the compatibility issues might be occurred after a fork. |

#### UncleBlock

| Name                    | Type          | Description                                                  |
| ----------------------- | ------------- | ------------------------------------------------------------ |
| `cellbase`              | `Transaction` | **The cellbase transaction of the uncle block.**             |
| `header`                | `Header`      | **The block header of the uncle block.**                     |
| `proposal_transactions` | [`string`]    | **An array of short transaction IDs of the transactions in the uncle block.** |

