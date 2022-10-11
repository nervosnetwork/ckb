use crate::error::RPCError;
use ckb_indexer::{
    service::{Cell, CellsCapacity, IndexerTip, Order, Pagination, SearchKey, Tx},
    IndexerHandle,
};
use ckb_jsonrpc_types::{JsonBytes, Uint32};
use jsonrpc_core::Result;
use jsonrpc_derive::rpc;

/// RPC Module Indexer.
#[rpc(server)]
pub trait IndexerRpc {
    /// Returns the indexed tip
    ///
    /// ## Returns
    ///   * block_hash - indexed tip block hash
    ///   * block_number - indexed tip block number
    ///
    /// ## Examples
    ///
    /// Request
    ///
    /// ```json
    /// {
    ///     "id": 2,
    ///     "jsonrpc": "2.0",
    ///     "method": "get_indexer_tip"
    /// }
    /// ```
    ///
    /// Response
    ///
    /// ```json
    /// {
    ///   "jsonrpc": "2.0",
    ///   "result": {
    ///     "block_hash": "0x4959d6e764a2edc6038dbf03d61ebcc99371115627b186fdcccb2161fbd26edc",
    ///     "block_number": "0x5b513e"
    ///   },
    ///   "id": 2
    /// }
    /// ```
    #[rpc(name = "get_indexer_tip")]
    fn get_indexer_tip(&self) -> Result<Option<IndexerTip>>;

    /// Returns the live cells collection by the lock or type script.
    ///
    /// ## Params
    ///
    /// * search_key:
    ///     - script - Script, supports prefix search
    ///     - scrip_type - enum, lock | type
    ///     - filter - filter cells by following conditions, all conditions are optional
    ///          - script: if search script type is lock, filter cells by type script prefix, and vice versa
    ///          - script_len_range: [u64; 2], filter cells by script len range, [inclusive, exclusive]
    ///          - output_data_len_range: [u64; 2], filter cells by output data len range, [inclusive, exclusive]
    ///          - output_capacity_range: [u64; 2], filter cells by output capacity range, [inclusive, exclusive]
    ///          - block_range: [u64; 2], filter cells by block number range, [inclusive, exclusive]
    ///     - with_data - bool, optional default is `true`, if with_data is set to false, the field of returning cell.output_data is null in the result
    /// * order: enum, asc | desc
    /// * limit: result size limit
    /// * after_cursor: pagination parameter, optional
    ///
    /// ## Returns
    ///
    /// * objects:
    ///     - output: the fields of an output cell
    ///     - output_data: the cell data
    ///     - out_point: reference to a cell via transaction hash and output index
    ///     - block_number: the number of the transaction committed in the block
    ///     - tx_index: the position index of the transaction committed in the block
    /// * last_cursor: pagination parameter
    ///
    /// ## Examples
    ///
    /// * get cells by lock script
    ///
    /// Request
    ///
    /// ```json
    /// {
    ///     "id": 2,
    ///     "jsonrpc": "2.0",
    ///     "method": "get_cells",
    ///     "params": [
    ///         {
    ///             "script": {
    ///                 "code_hash": "0x9bd7e06f3ecf4be0f2fcd2188b23f1b9fcc88e5d4b65a8637b17723bbda3cce8",
    ///                 "hash_type": "type",
    ///                 "args": "0x5989ae415bb667931a99896e5fbbfad9ba53a223"
    ///             },
    ///             "script_type": "lock"
    ///         },
    ///         "asc",
    ///         "0x64"
    ///     ]
    /// }
    /// ```
    ///
    /// Response
    ///
    /// ```json
    ///    {
    ///     "jsonrpc": "2.0",
    ///     "result": {
    ///       "last_cursor": "0x409bd7e06f3ecf4be0f2fcd2188b23f1b9fcc88e5d4b65a8637b17723bbda3cce8015989ae415bb667931a99896e5fbbfad9ba53a22300000000005b0f8c0000000100000000",
    ///       "objects": [
    ///         {
    ///           "block_number": "0x5b0e6d",
    ///           "out_point": {
    ///             "index": "0x0",
    ///             "tx_hash": "0xe8f2180dfba0cb15b45f771d520834515a5f8d7aa07f88894da88c22629b79e9"
    ///           },
    ///           "output": {
    ///             "capacity": "0x189640200",
    ///             "lock": {
    ///               "args": "0x5989ae415bb667931a99896e5fbbfad9ba53a223",
    ///               "code_hash": "0x9bd7e06f3ecf4be0f2fcd2188b23f1b9fcc88e5d4b65a8637b17723bbda3cce8",
    ///               "hash_type": "type"
    ///             },
    ///             "type": null
    ///           },
    ///           "output_data": "0x",
    ///           "tx_index": "0x1"
    ///         },
    ///         {
    ///           "block_number": "0x5b0e90",
    ///           "out_point": {
    ///             "index": "0x0",
    ///             "tx_hash": "0xece3a27409bde2914fb7a1555d6bfca453ee46af73e665149ef549fd46ec1fc6"
    ///           },
    ///           "output": {
    ///             "capacity": "0x189640200",
    ///             "lock": {
    ///               "args": "0x5989ae415bb667931a99896e5fbbfad9ba53a223",
    ///               "code_hash": "0x9bd7e06f3ecf4be0f2fcd2188b23f1b9fcc88e5d4b65a8637b17723bbda3cce8",
    ///               "hash_type": "type"
    ///             },
    ///             "type": null
    ///           },
    ///           "output_data": "0x",
    ///           "tx_index": "0x1"
    ///         },
    ///         {
    ///           "block_number": "0x5b0ead",
    ///           "out_point": {
    ///             "index": "0x1",
    ///             "tx_hash": "0x5c48768f91e3795b418c53211c76fd038c464a24c4aa7e35bbbb6ac5b219f581"
    ///           },
    ///           "output": {
    ///             "capacity": "0xe36dceec20",
    ///             "lock": {
    ///               "args": "0x5989ae415bb667931a99896e5fbbfad9ba53a223",
    ///               "code_hash": "0x9bd7e06f3ecf4be0f2fcd2188b23f1b9fcc88e5d4b65a8637b17723bbda3cce8",
    ///               "hash_type": "type"
    ///             },
    ///             "type": null
    ///           },
    ///           "output_data": "0x",
    ///           "tx_index": "0x1"
    ///         },
    ///         {
    ///           "block_number": "0x5b0eeb",
    ///           "out_point": {
    ///             "index": "0x0",
    ///             "tx_hash": "0x90e6981d6a5692d92e54344dc0e12d213447710fa069cc19ddea874619b9ba48"
    ///           },
    ///           "output": {
    ///             "capacity": "0x174876e800",
    ///             "lock": {
    ///               "args": "0x5989ae415bb667931a99896e5fbbfad9ba53a223",
    ///               "code_hash": "0x9bd7e06f3ecf4be0f2fcd2188b23f1b9fcc88e5d4b65a8637b17723bbda3cce8",
    ///               "hash_type": "type"
    ///             },
    ///             "type": null
    ///           },
    ///           "output_data": "0x",
    ///           "tx_index": "0x1"
    ///         },
    ///         {
    ///           "block_number": "0x5b0f8c",
    ///           "out_point": {
    ///             "index": "0x0",
    ///             "tx_hash": "0x9ea14510219ae97afa0275215fa77c3c015905281c953a3917a7fd036767429c"
    ///           },
    ///           "output": {
    ///             "capacity": "0x189640200",
    ///             "lock": {
    ///               "args": "0x5989ae415bb667931a99896e5fbbfad9ba53a223",
    ///               "code_hash": "0x9bd7e06f3ecf4be0f2fcd2188b23f1b9fcc88e5d4b65a8637b17723bbda3cce8",
    ///               "hash_type": "type"
    ///             },
    ///             "type": null
    ///           },
    ///           "output_data": "0x",
    ///           "tx_index": "0x1"
    ///         }
    ///       ]
    ///     },
    ///     "id": 2
    ///   }
    /// ```
    ///
    /// * get cells by lock script and filter by type script
    ///
    /// Request
    ///
    /// ```json
    /// {
    ///     "id": 2,
    ///     "jsonrpc": "2.0",
    ///     "method": "get_cells",
    ///     "params": [
    ///         {
    ///             "script": {
    ///                 "code_hash": "0x58c5f491aba6d61678b7cf7edf4910b1f5e00ec0cde2f42e0abb4fd9aff25a63",
    ///                 "hash_type": "type",
    ///                 "args": "0x2a49720e721553d0614dff29454ee4e1f07d0707"
    ///             },
    ///             "script_type": "lock",
    ///             "filter": {
    ///                 "script": {
    ///                     "code_hash": "0xc5e5dcf215925f7ef4dfaf5f4b4f105bc321c02776d6e7d52a1db3fcd9d011a4",
    ///                     "hash_type": "type",
    ///                     "args": "0x8462b20277bcbaa30d821790b852fb322d55c2b12e750ea91ad7059bc98dda4b"
    ///                 }
    ///             }
    ///         },
    ///         "asc",
    ///         "0x64"
    ///     ]
    /// }
    /// ```
    ///
    /// Response
    ///
    /// ```json
    /// {
    ///     "jsonrpc": "2.0",
    ///     "result": {
    ///       "last_cursor": "0x4058c5f491aba6d61678b7cf7edf4910b1f5e00ec0cde2f42e0abb4fd9aff25a63012a49720e721553d0614dff29454ee4e1f07d070700000000002adf870000000100000001",
    ///       "objects": [
    ///         {
    ///           "block_number": "0x2adf87",
    ///           "out_point": {
    ///             "index": "0x1",
    ///             "tx_hash": "0x04ecbc2df39e3682326a3b23c1bd2465e07eae2379ac0cc713834a1f79753779"
    ///           },
    ///           "output": {
    ///             "capacity": "0x436d81500",
    ///             "lock": {
    ///               "args": "0x2a49720e721553d0614dff29454ee4e1f07d0707",
    ///               "code_hash": "0x58c5f491aba6d61678b7cf7edf4910b1f5e00ec0cde2f42e0abb4fd9aff25a63",
    ///               "hash_type": "type"
    ///             },
    ///             "type": {
    ///               "args": "0x8462b20277bcbaa30d821790b852fb322d55c2b12e750ea91ad7059bc98dda4b",
    ///               "code_hash": "0xc5e5dcf215925f7ef4dfaf5f4b4f105bc321c02776d6e7d52a1db3fcd9d011a4",
    ///               "hash_type": "type"
    ///             }
    ///           },
    ///           "output_data": "0x0040d20853d746000000000000000000",
    ///           "tx_index": "0x1"
    ///         }
    ///       ]
    ///     },
    ///     "id": 2
    /// }
    /// ```
    ///
    /// * get cells by lock script and filter empty type script by setting script_len_range to
    /// [0, 1), script_len is caculated by (code_hash + hash_type + args).len
    ///
    /// Request
    ///
    /// ```json
    /// {
    ///     "id": 2,
    ///     "jsonrpc": "2.0",
    ///     "method": "get_cells",
    ///     "params": [
    ///         {
    ///             "script": {
    ///                 "code_hash": "0x58c5f491aba6d61678b7cf7edf4910b1f5e00ec0cde2f42e0abb4fd9aff25a63",
    ///                 "hash_type": "type",
    ///                 "args": "0x2a49720e721553d0614dff29454ee4e1f07d0707"
    ///             },
    ///             "script_type": "lock",
    ///             "filter": {
    ///                 "script_len_range": ["0x0", "0x1"]
    ///             }
    ///         },
    ///         "asc",
    ///         "0x64"
    ///     ]
    /// }
    /// ```
    ///
    /// Response
    ///
    /// ```json
    /// {
    ///     "jsonrpc": "2.0",
    ///     "result": {
    ///       "last_cursor": "0x4058c5f491aba6d61678b7cf7edf4910b1f5e00ec0cde2f42e0abb4fd9aff25a63012a49720e721553d0614dff29454ee4e1f07d070700000000002adf830000000200000001",
    ///       "objects": [
    ///         {
    ///           "block_number": "0x2adf83",
    ///           "out_point": {
    ///             "index": "0x1",
    ///             "tx_hash": "0x23ec897027c1d2a2b39e2446162bac182f18581be048cb3896ad695559b6839e"
    ///           },
    ///           "output": {
    ///             "capacity": "0x54b42b70b4",
    ///             "lock": {
    ///               "args": "0x2a49720e721553d0614dff29454ee4e1f07d0707",
    ///               "code_hash": "0x58c5f491aba6d61678b7cf7edf4910b1f5e00ec0cde2f42e0abb4fd9aff25a63",
    ///               "hash_type": "type"
    ///             },
    ///             "type": null
    ///           },
    ///           "output_data": "0x",
    ///           "tx_index": "0x2"
    ///         }
    ///       ]
    ///     },
    ///     "id": 2
    /// }
    /// ```
    ///
    /// * get cells by lock script and filter capacity range
    ///
    /// Request
    ///
    /// ```json
    /// {
    ///     "id": 2,
    ///     "jsonrpc": "2.0",
    ///     "method": "get_cells",
    ///     "params": [
    ///         {
    ///             "script": {
    ///                 "code_hash": "0x9bd7e06f3ecf4be0f2fcd2188b23f1b9fcc88e5d4b65a8637b17723bbda3cce8",
    ///                 "hash_type": "type",
    ///                 "args": "0x5989ae415bb667931a99896e5fbbfad9ba53a223"
    ///             },
    ///             "script_type": "lock",
    ///             "filter": {
    ///                 "output_capacity_range": ["0x0", "0x174876e801"]
    ///             }
    ///         },
    ///         "asc",
    ///         "0x64"
    ///     ]
    /// }
    /// ```
    ///
    /// Response
    ///
    /// ```json
    /// {
    ///     "jsonrpc": "2.0",
    ///     "result": {
    ///       "last_cursor": "0x409bd7e06f3ecf4be0f2fcd2188b23f1b9fcc88e5d4b65a8637b17723bbda3cce8015989ae415bb667931a99896e5fbbfad9ba53a22300000000005b59df0000000100000001",
    ///       "objects": [
    ///         {
    ///           "block_number": "0x5b59df",
    ///           "out_point": {
    ///             "index": "0x1",
    ///             "tx_hash": "0x21c4632a41140b828e9347ff80480b3e07be4e0a0b8d577565e7421fd5473194"
    ///           },
    ///           "output": {
    ///             "capacity": "0xe815b81c0",
    ///             "lock": {
    ///               "args": "0x5989ae415bb667931a99896e5fbbfad9ba53a223",
    ///               "code_hash": "0x9bd7e06f3ecf4be0f2fcd2188b23f1b9fcc88e5d4b65a8637b17723bbda3cce8",
    ///               "hash_type": "type"
    ///             },
    ///             "type": null
    ///           },
    ///           "output_data": "0x",
    ///           "tx_index": "0x1"
    ///         }
    ///       ]
    ///     },
    ///     "id": 2
    /// }
    /// ```
    #[rpc(name = "get_cells")]
    fn get_cells(
        &self,
        search_key: SearchKey,
        order: Order,
        limit: Uint32,
        after: Option<JsonBytes>,
    ) -> Result<Pagination<Cell>>;

    /// Returns the transactions collection by the lock or type script.
    ///
    /// * search_key:
    ///     - script - Script, supports prefix search when group_by_transaction is false
    ///     - scrip_type - enum, lock | type
    ///     - filter - filter cells by following conditions, all conditions are optional
    ///         - script: if search script type is lock, filter cells by type script, and vice versa
    ///         - block_range: [u64; 2], filter cells by block number range, [inclusive, exclusive]
    ///     - group_by_transaction - bool, optional default is `false`, if group_by_transaction is set to true, the returning objects will be grouped by the tx hash
    /// * order: enum, asc | desc
    /// * limit: result size limit
    /// * after_cursor: pagination parameter, optional
    ///
    /// ## Returns
    ///  * objects - enum, ungrouped TxWithCell | grouped TxWithCells
    ///     - TxWithCell:
    ///         - tx_hash: transaction hash,
    ///         - block_number: the number of the transaction committed in the block
    ///         - tx_index: the position index of the transaction committed in the block
    ///         - io_type: enum, input | output
    ///         - io_index: the position index of the cell in the transaction inputs or outputs
    ///     - TxWithCells:
    ///         - tx_hash: transaction hash,
    ///         - block_number: the number of the transaction committed in the block
    ///         - tx_index: the position index of the transaction committed in the block
    ///         - cells: Array [[io_type, io_index]]
    ///  * last_cursor - pagination parameter
    ///
    /// ## Examples
    ///
    /// * get transactions by lock script
    ///
    /// Request
    ///
    /// ```json
    /// {
    ///     "id": 2,
    ///     "jsonrpc": "2.0",
    ///     "method": "get_transactions",
    ///     "params": [
    ///         {
    ///             "script": {
    ///                 "code_hash": "0x9bd7e06f3ecf4be0f2fcd2188b23f1b9fcc88e5d4b65a8637b17723bbda3cce8",
    ///                 "hash_type": "type",
    ///                 "args": "0x5989ae415bb667931a99896e5fbbfad9ba53a223"
    ///             },
    ///             "script_type": "lock"
    ///         },
    ///         "asc",
    ///         "0x64"
    ///     ]
    /// }
    /// ```
    ///
    /// Response
    ///
    /// ```json
    /// {
    ///     "jsonrpc": "2.0",
    ///     "result": {
    ///       "last_cursor": "0x809bd7e06f3ecf4be0f2fcd2188b23f1b9fcc88e5d4b65a8637b17723bbda3cce8015989ae415bb667931a99896e5fbbfad9ba53a22300000000005b59df000000010000000101",
    ///       "objects": [
    ///         {
    ///           "block_number": "0x5b033a",
    ///           "io_index": "0x0",
    ///           "io_type": "output",
    ///           "tx_hash": "0x556060b62d16386da53f8a4b458314dfa2d1988a7bcc5c96c3bb2a350a3453a1",
    ///           "tx_index": "0x4"
    ///         },
    ///         {
    ///           "block_number": "0x5b0671",
    ///           "io_index": "0x0",
    ///           "io_type": "input",
    ///           "tx_hash": "0x8205b2b4cd6380d7e332c7a5b49bf776a0322ba19f46dc6ca1f8c59f7daee08d",
    ///           "tx_index": "0x1"
    ///         },
    ///         {
    ///           "block_number": "0x5b0671",
    ///           "io_index": "0x1",
    ///           "io_type": "output",
    ///           "tx_hash": "0x8205b2b4cd6380d7e332c7a5b49bf776a0322ba19f46dc6ca1f8c59f7daee08d",
    ///           "tx_index": "0x1"
    ///         },
    ///         {
    ///           "block_number": "0x5b0e6d",
    ///           "io_index": "0x0",
    ///           "io_type": "output",
    ///           "tx_hash": "0xe8f2180dfba0cb15b45f771d520834515a5f8d7aa07f88894da88c22629b79e9",
    ///           "tx_index": "0x1"
    ///         },
    ///         {
    ///           "block_number": "0x5b0e90",
    ///           "io_index": "0x0",
    ///           "io_type": "output",
    ///           "tx_hash": "0xece3a27409bde2914fb7a1555d6bfca453ee46af73e665149ef549fd46ec1fc6",
    ///           "tx_index": "0x1"
    ///         },
    ///         {
    ///           "block_number": "0x5b0ead",
    ///           "io_index": "0x0",
    ///           "io_type": "input",
    ///           "tx_hash": "0x5c48768f91e3795b418c53211c76fd038c464a24c4aa7e35bbbb6ac5b219f581",
    ///           "tx_index": "0x1"
    ///         },
    ///         {
    ///           "block_number": "0x5b0ead",
    ///           "io_index": "0x1",
    ///           "io_type": "output",
    ///           "tx_hash": "0x5c48768f91e3795b418c53211c76fd038c464a24c4aa7e35bbbb6ac5b219f581",
    ///           "tx_index": "0x1"
    ///         },
    ///         {
    ///           "block_number": "0x5b0eeb",
    ///           "io_index": "0x0",
    ///           "io_type": "output",
    ///           "tx_hash": "0x90e6981d6a5692d92e54344dc0e12d213447710fa069cc19ddea874619b9ba48",
    ///           "tx_index": "0x1"
    ///         },
    ///         {
    ///           "block_number": "0x5b0f8c",
    ///           "io_index": "0x0",
    ///           "io_type": "output",
    ///           "tx_hash": "0x9ea14510219ae97afa0275215fa77c3c015905281c953a3917a7fd036767429c",
    ///           "tx_index": "0x1"
    ///         },
    ///         {
    ///           "block_number": "0x5b5638",
    ///           "io_index": "0x0",
    ///           "io_type": "input",
    ///           "tx_hash": "0x9346da4caa846cc035c182ecad0c17326a587983d25fb1e12a388f1a9c5c56b4",
    ///           "tx_index": "0x1"
    ///         },
    ///         {
    ///           "block_number": "0x5b5638",
    ///           "io_index": "0x1",
    ///           "io_type": "input",
    ///           "tx_hash": "0x9346da4caa846cc035c182ecad0c17326a587983d25fb1e12a388f1a9c5c56b4",
    ///           "tx_index": "0x1"
    ///         },
    ///         {
    ///           "block_number": "0x5b5638",
    ///           "io_index": "0x1",
    ///           "io_type": "output",
    ///           "tx_hash": "0x9346da4caa846cc035c182ecad0c17326a587983d25fb1e12a388f1a9c5c56b4",
    ///           "tx_index": "0x1"
    ///         },
    ///         {
    ///           "block_number": "0x5b5638",
    ///           "io_index": "0x2",
    ///           "io_type": "input",
    ///           "tx_hash": "0x9346da4caa846cc035c182ecad0c17326a587983d25fb1e12a388f1a9c5c56b4",
    ///           "tx_index": "0x1"
    ///         },
    ///         {
    ///           "block_number": "0x5b59c2",
    ///           "io_index": "0x0",
    ///           "io_type": "input",
    ///           "tx_hash": "0x5b58f90fb3309333bf0bec878f3a05038c7fe816747300ecdac37a9da76c4128",
    ///           "tx_index": "0x1"
    ///         },
    ///         {
    ///           "block_number": "0x5b59c2",
    ///           "io_index": "0x1",
    ///           "io_type": "output",
    ///           "tx_hash": "0x5b58f90fb3309333bf0bec878f3a05038c7fe816747300ecdac37a9da76c4128",
    ///           "tx_index": "0x1"
    ///         },
    ///         {
    ///           "block_number": "0x5b59cc",
    ///           "io_index": "0x0",
    ///           "io_type": "input",
    ///           "tx_hash": "0x57ca2822c28e02b199424a731b2efd2c9bf752f07b7309f555f2e71abe83ba26",
    ///           "tx_index": "0x1"
    ///         },
    ///         {
    ///           "block_number": "0x5b59cc",
    ///           "io_index": "0x1",
    ///           "io_type": "input",
    ///           "tx_hash": "0x57ca2822c28e02b199424a731b2efd2c9bf752f07b7309f555f2e71abe83ba26",
    ///           "tx_index": "0x1"
    ///         },
    ///         {
    ///           "block_number": "0x5b59cc",
    ///           "io_index": "0x1",
    ///           "io_type": "output",
    ///           "tx_hash": "0x57ca2822c28e02b199424a731b2efd2c9bf752f07b7309f555f2e71abe83ba26",
    ///           "tx_index": "0x1"
    ///         },
    ///         {
    ///           "block_number": "0x5b59df",
    ///           "io_index": "0x0",
    ///           "io_type": "input",
    ///           "tx_hash": "0x21c4632a41140b828e9347ff80480b3e07be4e0a0b8d577565e7421fd5473194",
    ///           "tx_index": "0x1"
    ///         },
    ///         {
    ///           "block_number": "0x5b59df",
    ///           "io_index": "0x1",
    ///           "io_type": "output",
    ///           "tx_hash": "0x21c4632a41140b828e9347ff80480b3e07be4e0a0b8d577565e7421fd5473194",
    ///           "tx_index": "0x1"
    ///         }
    ///       ]
    ///     },
    ///     "id": 2
    /// }
    /// ```
    ///
    /// * get transactions by lock script and group by tx hash
    ///
    /// Request
    ///
    /// ```json
    /// {
    ///     "id": 2,
    ///     "jsonrpc": "2.0",
    ///     "method": "get_transactions",
    ///     "params": [
    ///         {
    ///             "script": {
    ///                 "code_hash": "0x9bd7e06f3ecf4be0f2fcd2188b23f1b9fcc88e5d4b65a8637b17723bbda3cce8",
    ///                 "hash_type": "type",
    ///                 "args": "0x5989ae415bb667931a99896e5fbbfad9ba53a223"
    ///             },
    ///             "script_type": "lock",
    ///             "group_by_transaction": true
    ///         },
    ///         "asc",
    ///         "0x64"
    ///     ]
    /// }
    /// ```
    ///
    /// Response
    ///
    /// ```json
    /// {
    ///     "jsonrpc": "2.0",
    ///     "result": {
    ///       "last_cursor": "0x809bd7e06f3ecf4be0f2fcd2188b23f1b9fcc88e5d4b65a8637b17723bbda3cce8015989ae415bb667931a99896e5fbbfad9ba53a22300000000005b59df000000010000000101",
    ///       "objects": [
    ///         {
    ///           "block_number": "0x5b033a",
    ///           "cells": [
    ///             [
    ///               "output",
    ///               "0x0"
    ///             ]
    ///           ],
    ///           "tx_hash": "0x556060b62d16386da53f8a4b458314dfa2d1988a7bcc5c96c3bb2a350a3453a1",
    ///           "tx_index": "0x4"
    ///         },
    ///         {
    ///           "block_number": "0x5b0671",
    ///           "cells": [
    ///             [
    ///               "input",
    ///               "0x0"
    ///             ],
    ///             [
    ///               "output",
    ///               "0x1"
    ///             ]
    ///           ],
    ///           "tx_hash": "0x8205b2b4cd6380d7e332c7a5b49bf776a0322ba19f46dc6ca1f8c59f7daee08d",
    ///           "tx_index": "0x1"
    ///         },
    ///         {
    ///           "block_number": "0x5b0e6d",
    ///           "cells": [
    ///             [
    ///               "output",
    ///               "0x0"
    ///             ]
    ///           ],
    ///           "tx_hash": "0xe8f2180dfba0cb15b45f771d520834515a5f8d7aa07f88894da88c22629b79e9",
    ///           "tx_index": "0x1"
    ///         },
    ///         {
    ///           "block_number": "0x5b0e90",
    ///           "cells": [
    ///             [
    ///               "output",
    ///               "0x0"
    ///             ]
    ///           ],
    ///           "tx_hash": "0xece3a27409bde2914fb7a1555d6bfca453ee46af73e665149ef549fd46ec1fc6",
    ///           "tx_index": "0x1"
    ///         },
    ///         {
    ///           "block_number": "0x5b0ead",
    ///           "cells": [
    ///             [
    ///               "input",
    ///               "0x0"
    ///             ],
    ///             [
    ///               "output",
    ///               "0x1"
    ///             ]
    ///           ],
    ///           "tx_hash": "0x5c48768f91e3795b418c53211c76fd038c464a24c4aa7e35bbbb6ac5b219f581",
    ///           "tx_index": "0x1"
    ///         },
    ///         {
    ///           "block_number": "0x5b0eeb",
    ///           "cells": [
    ///             [
    ///               "output",
    ///               "0x0"
    ///             ]
    ///           ],
    ///           "tx_hash": "0x90e6981d6a5692d92e54344dc0e12d213447710fa069cc19ddea874619b9ba48",
    ///           "tx_index": "0x1"
    ///         },
    ///         {
    ///           "block_number": "0x5b0f8c",
    ///           "cells": [
    ///             [
    ///               "output",
    ///               "0x0"
    ///             ]
    ///           ],
    ///           "tx_hash": "0x9ea14510219ae97afa0275215fa77c3c015905281c953a3917a7fd036767429c",
    ///           "tx_index": "0x1"
    ///         },
    ///         {
    ///           "block_number": "0x5b5638",
    ///           "cells": [
    ///             [
    ///               "input",
    ///               "0x0"
    ///             ],
    ///             [
    ///               "input",
    ///               "0x1"
    ///             ],
    ///             [
    ///               "output",
    ///               "0x1"
    ///             ],
    ///             [
    ///               "input",
    ///               "0x2"
    ///             ]
    ///           ],
    ///           "tx_hash": "0x9346da4caa846cc035c182ecad0c17326a587983d25fb1e12a388f1a9c5c56b4",
    ///           "tx_index": "0x1"
    ///         },
    ///         {
    ///           "block_number": "0x5b59c2",
    ///           "cells": [
    ///             [
    ///               "input",
    ///               "0x0"
    ///             ],
    ///             [
    ///               "output",
    ///               "0x1"
    ///             ]
    ///           ],
    ///           "tx_hash": "0x5b58f90fb3309333bf0bec878f3a05038c7fe816747300ecdac37a9da76c4128",
    ///           "tx_index": "0x1"
    ///         },
    ///         {
    ///           "block_number": "0x5b59cc",
    ///           "cells": [
    ///             [
    ///               "input",
    ///               "0x0"
    ///             ],
    ///             [
    ///               "input",
    ///               "0x1"
    ///             ],
    ///             [
    ///               "output",
    ///               "0x1"
    ///             ]
    ///           ],
    ///           "tx_hash": "0x57ca2822c28e02b199424a731b2efd2c9bf752f07b7309f555f2e71abe83ba26",
    ///           "tx_index": "0x1"
    ///         },
    ///         {
    ///           "block_number": "0x5b59df",
    ///           "cells": [
    ///             [
    ///               "input",
    ///               "0x0"
    ///             ],
    ///             [
    ///               "output",
    ///               "0x1"
    ///             ]
    ///           ],
    ///           "tx_hash": "0x21c4632a41140b828e9347ff80480b3e07be4e0a0b8d577565e7421fd5473194",
    ///           "tx_index": "0x1"
    ///         }
    ///       ]
    ///     },
    ///     "id": 2
    /// }
    /// ```
    #[rpc(name = "get_transactions")]
    fn get_transactions(
        &self,
        search_key: SearchKey,
        order: Order,
        limit: Uint32,
        after: Option<JsonBytes>,
    ) -> Result<Pagination<Tx>>;

    /// Returns the live cells capacity by the lock or type script.
    ///
    /// ## Parameters
    ///
    /// * search_key:
    ///     - script - Script
    ///     - scrip_type - enum, lock | type
    ///     - filter - filter cells by following conditions, all conditions are optional
    ///         - script: if search script type is lock, filter cells by type script prefix, and vice versa
    ///         - output_data_len_range: [u64; 2], filter cells by output data len range, [inclusive, exclusive]
    ///         - output_capacity_range: [u64; 2], filter cells by output capacity range, [inclusive, exclusive]
    ///         - block_range: [u64; 2], filter cells by block number range, [inclusive, exclusive]
    ///
    /// ## Returns
    ///
    ///  * capacity - total capacity
    ///  * block_hash - indexed tip block hash
    ///  * block_number - indexed tip block number
    ///
    /// ## Examples
    ///
    /// Request
    ///
    /// ```json
    /// {
    ///     "id": 2,
    ///     "jsonrpc": "2.0",
    ///     "method": "get_cells_capacity",
    ///     "params": [
    ///         {
    ///             "script": {
    ///                 "code_hash": "0x9bd7e06f3ecf4be0f2fcd2188b23f1b9fcc88e5d4b65a8637b17723bbda3cce8",
    ///                 "hash_type": "type",
    ///                 "args": "0x5989ae415bb667931a99896e5fbbfad9ba53a223"
    ///             },
    ///             "script_type": "lock"
    ///         }
    ///     ]
    /// }
    /// ```
    ///
    /// Response
    ///
    /// ```json
    /// {
    ///     "jsonrpc": "2.0",
    ///     "result": {
    ///       "block_hash": "0xbc52444952dc5eb01a7826aaf6bb1b660db01797414e259e7a6e6d636de8fc7c",
    ///       "block_number": "0x5b727a",
    ///       "capacity": "0xf0e8e4b4a0"
    ///     },
    ///     "id": 2
    /// }
    /// ```
    #[rpc(name = "get_cells_capacity")]
    fn get_cells_capacity(&self, search_key: SearchKey) -> Result<Option<CellsCapacity>>;
}

pub(crate) struct IndexerRpcImpl {
    pub(crate) handle: IndexerHandle,
}

impl IndexerRpcImpl {
    pub fn new(handle: IndexerHandle) -> Self {
        IndexerRpcImpl { handle }
    }
}

impl IndexerRpc for IndexerRpcImpl {
    fn get_indexer_tip(&self) -> Result<Option<IndexerTip>> {
        self.handle
            .get_indexer_tip()
            .map_err(|e| RPCError::custom(RPCError::Indexer, e))
    }

    fn get_cells(
        &self,
        search_key: SearchKey,
        order: Order,
        limit: Uint32,
        after: Option<JsonBytes>,
    ) -> Result<Pagination<Cell>> {
        self.handle
            .get_cells(search_key, order, limit, after)
            .map_err(|e| RPCError::custom(RPCError::Indexer, e))
    }

    fn get_transactions(
        &self,
        search_key: SearchKey,
        order: Order,
        limit: Uint32,
        after: Option<JsonBytes>,
    ) -> Result<Pagination<Tx>> {
        self.handle
            .get_transactions(search_key, order, limit, after)
            .map_err(|e| RPCError::custom(RPCError::Indexer, e))
    }

    fn get_cells_capacity(&self, search_key: SearchKey) -> Result<Option<CellsCapacity>> {
        self.handle
            .get_cells_capacity(search_key)
            .map_err(|e| RPCError::custom(RPCError::Indexer, e))
    }
}
