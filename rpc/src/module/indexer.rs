use ckb_indexer::IndexerStore;
use ckb_jsonrpc_types::{
    BlockNumber, CellTransaction, LiveCell, LockHashCapacity, LockHashIndexState, Uint64,
};
use ckb_types::{prelude::*, H256};
use jsonrpc_core::Result;
use jsonrpc_derive::rpc;

/// RPC Module Indexer which index cells by lock script hash.
///
/// The index is disabled by default, which **must** be enabled by calling [`index_lock_hash`](#tymethod.index_lock_hash) first.
#[deprecated(
    since = "0.36.0",
    note = "Please use [ckb-indexer](https://github.com/nervosnetwork/ckb-indexer) as an alternate solution."
)]
#[rpc(server)]
pub trait IndexerRpc {
    /// Returns the live cells collection by the hash of lock script.
    ///
    /// This RPC requires [creating the index](#tymethod.index_lock_hash) on `lock_hash` first.
    /// It returns all live cells only if the index is created starting from the genesis block.
    ///
    /// ## Params
    ///
    /// * `lock_hash` - Cell lock script hash
    /// * `page` - Page number, starting from 0
    /// * `per` - Page size, max value is 50
    /// * `reverse_order` - Returns the live cells collection in reverse order. (**Optional**, default is false)
    ///
    /// ## Examples
    ///
    /// Request
    ///
    /// ```json
    /// {
    ///   "id": 42,
    ///   "jsonrpc": "2.0",
    ///   "method": "get_live_cells_by_lock_hash",
    ///   "params": [
    ///     "0x4ceaa32f692948413e213ce6f3a83337145bde6e11fd8cb94377ce2637dcc412",
    ///     "0xa",
    ///     "0xe"
    ///   ]
    /// }
    /// ```
    ///
    /// Response
    ///
    /// ```json
    /// {
    ///   "id": 42,
    ///   "jsonrpc": "2.0",
    ///   "result": [
    ///     {
    ///       "cell_output": {
    ///         "capacity": "0x2cb6562e4e",
    ///         "lock": {
    ///           "args": "0x",
    ///           "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
    ///           "hash_type": "data"
    ///         },
    ///         "type": null
    ///       },
    ///       "cellbase": true,
    ///       "created_by": {
    ///         "block_number": "0x98",
    ///         "index": "0x0",
    ///         "tx_hash": "0x2d811f9ad7f2f7319171a6da4c842dd78e36682b4ac74da4f67b97c9f7d7a02b"
    ///       },
    ///       "output_data_len": "0x0"
    ///     },
    ///     {
    ///       "cell_output": {
    ///         "capacity": "0x2cb66b2496",
    ///         "lock": {
    ///           "args": "0x",
    ///           "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
    ///           "hash_type": "data"
    ///         },
    ///         "type": null
    ///       },
    ///       "cellbase": true,
    ///       "created_by": {
    ///         "block_number": "0x99",
    ///         "index": "0x0",
    ///         "tx_hash": "0x1ccf68bf7cb96a1a7f992c27bcfea6ebfc0fe32602196569aaa0cb3cd3e9f5ea"
    ///       },
    ///       "output_data_len": "0x0"
    ///     },
    ///     {
    ///       "cell_output": {
    ///         "capacity": "0x2cb68006e8",
    ///         "lock": {
    ///           "args": "0x",
    ///           "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
    ///           "hash_type": "data"
    ///         },
    ///         "type": null
    ///       },
    ///       "cellbase": true,
    ///       "created_by": {
    ///         "block_number": "0x9a",
    ///         "index": "0x0",
    ///         "tx_hash": "0x74db38ad40184dd0528f4841e10599ff97bfbf2b5313754d1e96920d8523a5d4"
    ///       },
    ///       "output_data_len": "0x0"
    ///     },
    ///     {
    ///       "cell_output": {
    ///         "capacity": "0x2cb694d55e",
    ///         "lock": {
    ///           "args": "0x",
    ///           "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
    ///           "hash_type": "data"
    ///         },
    ///         "type": null
    ///       },
    ///       "cellbase": true,
    ///       "created_by": {
    ///         "block_number": "0x9b",
    ///         "index": "0x0",
    ///         "tx_hash": "0xf7d0ecc70015b46c5ab1cc8462592ae612fdaada200f643f3e1ce633bcc5ad1d"
    ///       },
    ///       "output_data_len": "0x0"
    ///     },
    ///     {
    ///       "cell_output": {
    ///         "capacity": "0x2cb6a99016",
    ///         "lock": {
    ///           "args": "0x",
    ///           "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
    ///           "hash_type": "data"
    ///         },
    ///         "type": null
    ///       },
    ///       "cellbase": true,
    ///       "created_by": {
    ///         "block_number": "0x9c",
    ///         "index": "0x0",
    ///         "tx_hash": "0xc3d232bb6b0e5d9a71a0978c9ab66c7a127ed37aeed6a2509dcc10d994c8c605"
    ///       },
    ///       "output_data_len": "0x0"
    ///     },
    ///     {
    ///       "cell_output": {
    ///         "capacity": "0x2cb6be372c",
    ///         "lock": {
    ///           "args": "0x",
    ///           "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
    ///           "hash_type": "data"
    ///         },
    ///         "type": null
    ///       },
    ///       "cellbase": true,
    ///       "created_by": {
    ///         "block_number": "0x9d",
    ///         "index": "0x0",
    ///         "tx_hash": "0x10139a08beae170a35fbfcece6d50561ec61e13e4c6438435c1f2021331d7c4d"
    ///       },
    ///       "output_data_len": "0x0"
    ///     },
    ///     {
    ///       "cell_output": {
    ///         "capacity": "0x2cb6d2cabb",
    ///         "lock": {
    ///           "args": "0x",
    ///           "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
    ///           "hash_type": "data"
    ///         },
    ///         "type": null
    ///       },
    ///       "cellbase": true,
    ///       "created_by": {
    ///         "block_number": "0x9e",
    ///         "index": "0x0",
    ///         "tx_hash": "0x39a083a1deb39b923a600a6f0714663085b5d2011b886b160962e20f1a28b550"
    ///       },
    ///       "output_data_len": "0x0"
    ///     },
    ///     {
    ///       "cell_output": {
    ///         "capacity": "0x2cb6e74ae0",
    ///         "lock": {
    ///           "args": "0x",
    ///           "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
    ///           "hash_type": "data"
    ///         },
    ///         "type": null
    ///       },
    ///       "cellbase": true,
    ///       "created_by": {
    ///         "block_number": "0x9f",
    ///         "index": "0x0",
    ///         "tx_hash": "0x2899c066f80a04b9a168e4499760ad1d768f44a3d673779905d88edd86362ac6"
    ///       },
    ///       "output_data_len": "0x0"
    ///     },
    ///     {
    ///       "cell_output": {
    ///         "capacity": "0x2cb6fbb7b4",
    ///         "lock": {
    ///           "args": "0x",
    ///           "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
    ///           "hash_type": "data"
    ///         },
    ///         "type": null
    ///       },
    ///       "cellbase": true,
    ///       "created_by": {
    ///         "block_number": "0xa0",
    ///         "index": "0x0",
    ///         "tx_hash": "0xe2579280875a5d14538b0cc2356707792189662d5f8292541d9856ef291e81bf"
    ///       },
    ///       "output_data_len": "0x0"
    ///     },
    ///     {
    ///       "cell_output": {
    ///         "capacity": "0x2cb7101155",
    ///         "lock": {
    ///           "args": "0x",
    ///           "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
    ///           "hash_type": "data"
    ///         },
    ///         "type": null
    ///       },
    ///       "cellbase": true,
    ///       "created_by": {
    ///         "block_number": "0xa1",
    ///         "index": "0x0",
    ///         "tx_hash": "0xd6121e80237c79182d55ec0efb9fa75bc9cc592f818057ced51aac6bb625e016"
    ///       },
    ///       "output_data_len": "0x0"
    ///     },
    ///     {
    ///       "cell_output": {
    ///         "capacity": "0x2cb72457dc",
    ///         "lock": {
    ///           "args": "0x",
    ///           "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
    ///           "hash_type": "data"
    ///         },
    ///         "type": null
    ///       },
    ///       "cellbase": true,
    ///       "created_by": {
    ///         "block_number": "0xa2",
    ///         "index": "0x0",
    ///         "tx_hash": "0x624eba1135e54a5988cb2ec70d42fa860d1d5658ed7f8d402615dff7d598e4b6"
    ///       },
    ///       "output_data_len": "0x0"
    ///     },
    ///     {
    ///       "cell_output": {
    ///         "capacity": "0x2cb7388b65",
    ///         "lock": {
    ///           "args": "0x",
    ///           "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
    ///           "hash_type": "data"
    ///         },
    ///         "type": null
    ///       },
    ///       "cellbase": true,
    ///       "created_by": {
    ///         "block_number": "0xa3",
    ///         "index": "0x0",
    ///         "tx_hash": "0x7884b4cf85bc02cb73ec41d5cbbbf158eebca6ef855419ce57ff7c1d97b5be58"
    ///       },
    ///       "output_data_len": "0x0"
    ///     },
    ///     {
    ///       "cell_output": {
    ///         "capacity": "0x2cb74cac0a",
    ///         "lock": {
    ///           "args": "0x",
    ///           "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
    ///           "hash_type": "data"
    ///         },
    ///         "type": null
    ///       },
    ///       "cellbase": true,
    ///       "created_by": {
    ///         "block_number": "0xa4",
    ///         "index": "0x0",
    ///         "tx_hash": "0xb613ba9b5f6177657493492dd523a63720d855ae9749887a0de881b894a1d6a6"
    ///       },
    ///       "output_data_len": "0x0"
    ///     },
    ///     {
    ///       "cell_output": {
    ///         "capacity": "0x2cb760b9e6",
    ///         "lock": {
    ///           "args": "0x",
    ///           "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
    ///           "hash_type": "data"
    ///         },
    ///         "type": null
    ///       },
    ///       "cellbase": true,
    ///       "created_by": {
    ///         "block_number": "0xa5",
    ///         "index": "0x0",
    ///         "tx_hash": "0x701f4b962c1650810800ee6ed981841692c1939a4b597e9e7a726c5db77f6164"
    ///       },
    ///       "output_data_len": "0x0"
    ///     }
    ///   ]
    /// }
    /// ```
    #[rpc(name = "deprecated.get_live_cells_by_lock_hash")]
    fn get_live_cells_by_lock_hash(
        &self,
        lock_hash: H256,
        page: Uint64,
        per_page: Uint64,
        reverse_order: Option<bool>,
    ) -> Result<Vec<LiveCell>>;

    /// Returns the transactions collection by the hash of lock script.
    ///
    /// This RPC requires [creating the index](#tymethod.index_lock_hash) on `lock_hash` first.
    /// It returns all matched transactions only if the index is created starting from the genesis block.
    ///
    /// ## Params
    ///
    /// * `lock_hash` - Cell lock script hash
    /// * `page` - Page number, starting from 0
    /// * `per` - Page size, max value is 50
    /// * `reverse_order` - Return the transactions collection in reverse order. (**Optional**, default is false)
    ///
    /// ## Examples
    ///
    /// Request
    ///
    /// ```json
    /// {
    ///   "id": 42,
    ///   "jsonrpc": "2.0",
    ///   "method": "get_transactions_by_lock_hash",
    ///   "params": [
    ///     "0x4ceaa32f692948413e213ce6f3a83337145bde6e11fd8cb94377ce2637dcc412",
    ///     "0xa",
    ///     "0xe"
    ///   ]
    /// }
    /// ```
    ///
    /// Response
    ///
    /// ```json
    /// {
    ///   "id": 42,
    ///   "jsonrpc": "2.0",
    ///   "result": [
    ///     {
    ///       "consumed_by": null,
    ///       "created_by": {
    ///         "block_number": "0x98",
    ///         "index": "0x0",
    ///         "tx_hash": "0x2d811f9ad7f2f7319171a6da4c842dd78e36682b4ac74da4f67b97c9f7d7a02b"
    ///       }
    ///     },
    ///     {
    ///       "consumed_by": null,
    ///       "created_by": {
    ///         "block_number": "0x99",
    ///         "index": "0x0",
    ///         "tx_hash": "0x1ccf68bf7cb96a1a7f992c27bcfea6ebfc0fe32602196569aaa0cb3cd3e9f5ea"
    ///       }
    ///     },
    ///     {
    ///       "consumed_by": null,
    ///       "created_by": {
    ///         "block_number": "0x9a",
    ///         "index": "0x0",
    ///         "tx_hash": "0x74db38ad40184dd0528f4841e10599ff97bfbf2b5313754d1e96920d8523a5d4"
    ///       }
    ///     },
    ///     {
    ///       "consumed_by": null,
    ///       "created_by": {
    ///         "block_number": "0x9b",
    ///         "index": "0x0",
    ///         "tx_hash": "0xf7d0ecc70015b46c5ab1cc8462592ae612fdaada200f643f3e1ce633bcc5ad1d"
    ///       }
    ///     },
    ///     {
    ///       "consumed_by": null,
    ///       "created_by": {
    ///         "block_number": "0x9c",
    ///         "index": "0x0",
    ///         "tx_hash": "0xc3d232bb6b0e5d9a71a0978c9ab66c7a127ed37aeed6a2509dcc10d994c8c605"
    ///       }
    ///     },
    ///     {
    ///       "consumed_by": null,
    ///       "created_by": {
    ///         "block_number": "0x9d",
    ///         "index": "0x0",
    ///         "tx_hash": "0x10139a08beae170a35fbfcece6d50561ec61e13e4c6438435c1f2021331d7c4d"
    ///       }
    ///     },
    ///     {
    ///       "consumed_by": null,
    ///       "created_by": {
    ///         "block_number": "0x9e",
    ///         "index": "0x0",
    ///         "tx_hash": "0x39a083a1deb39b923a600a6f0714663085b5d2011b886b160962e20f1a28b550"
    ///       }
    ///     },
    ///     {
    ///       "consumed_by": null,
    ///       "created_by": {
    ///         "block_number": "0x9f",
    ///         "index": "0x0",
    ///         "tx_hash": "0x2899c066f80a04b9a168e4499760ad1d768f44a3d673779905d88edd86362ac6"
    ///       }
    ///     },
    ///     {
    ///       "consumed_by": null,
    ///       "created_by": {
    ///         "block_number": "0xa0",
    ///         "index": "0x0",
    ///         "tx_hash": "0xe2579280875a5d14538b0cc2356707792189662d5f8292541d9856ef291e81bf"
    ///       }
    ///     },
    ///     {
    ///       "consumed_by": null,
    ///       "created_by": {
    ///         "block_number": "0xa1",
    ///         "index": "0x0",
    ///         "tx_hash": "0xd6121e80237c79182d55ec0efb9fa75bc9cc592f818057ced51aac6bb625e016"
    ///       }
    ///     },
    ///     {
    ///       "consumed_by": null,
    ///       "created_by": {
    ///         "block_number": "0xa2",
    ///         "index": "0x0",
    ///         "tx_hash": "0x624eba1135e54a5988cb2ec70d42fa860d1d5658ed7f8d402615dff7d598e4b6"
    ///       }
    ///     },
    ///     {
    ///       "consumed_by": null,
    ///       "created_by": {
    ///         "block_number": "0xa3",
    ///         "index": "0x0",
    ///         "tx_hash": "0x7884b4cf85bc02cb73ec41d5cbbbf158eebca6ef855419ce57ff7c1d97b5be58"
    ///       }
    ///     },
    ///     {
    ///       "consumed_by": null,
    ///       "created_by": {
    ///         "block_number": "0xa4",
    ///         "index": "0x0",
    ///         "tx_hash": "0xb613ba9b5f6177657493492dd523a63720d855ae9749887a0de881b894a1d6a6"
    ///       }
    ///     },
    ///     {
    ///       "consumed_by": null,
    ///       "created_by": {
    ///         "block_number": "0xa5",
    ///         "index": "0x0",
    ///         "tx_hash": "0x701f4b962c1650810800ee6ed981841692c1939a4b597e9e7a726c5db77f6164"
    ///       }
    ///     }
    ///   ]
    /// }
    /// ```
    #[rpc(name = "deprecated.get_transactions_by_lock_hash")]
    fn get_transactions_by_lock_hash(
        &self,
        lock_hash: H256,
        page: Uint64,
        per_page: Uint64,
        reverse_order: Option<bool>,
    ) -> Result<Vec<CellTransaction>>;

    /// Creates index for live cells and transactions by the hash of lock script.
    ///
    /// The indices are disabled by default. Clients have to create indices first before querying.
    ///
    /// Creating index for the same `lock_hash` with different `index_from` is an undefined
    /// behaviour. Please [delete the index](#tymethod.deindex_lock_hash) first.
    ///
    /// ## Params
    ///
    /// * `lock_hash` - Cell lock script hash
    /// * `index_from` - Create an index starting from this block number (exclusive). 0 is special
    /// which also indexes transactions in the genesis block. (**Optional**, the default is the max
    /// block number in the canonical chain, which means starting index from the next new block.)
    ///
    /// ## Examples
    ///
    /// Request
    ///
    /// ```json
    /// {
    ///   "id": 42,
    ///   "jsonrpc": "2.0",
    ///   "method": "index_lock_hash",
    ///   "params": [
    ///     "0x4ceaa32f692948413e213ce6f3a83337145bde6e11fd8cb94377ce2637dcc412",
    ///     "0x400"
    ///   ]
    /// }
    /// ```
    ///
    /// Response
    ///
    /// ```json
    /// {
    ///   "id": 42,
    ///   "jsonrpc": "2.0",
    ///   "result": {
    ///     "block_hash": "0xa5f5c85987a15de25661e5a214f2c1449cd803f071acc7999820f25246471f40",
    ///     "block_number": "0x400",
    ///     "lock_hash": "0x4ceaa32f692948413e213ce6f3a83337145bde6e11fd8cb94377ce2637dcc412"
    ///   }
    /// }
    /// ```
    #[rpc(name = "deprecated.index_lock_hash")]
    fn index_lock_hash(
        &self,
        lock_hash: H256,
        index_from: Option<BlockNumber>,
    ) -> Result<LockHashIndexState>;

    /// Removes index for live cells and transactions by the hash of lock script.
    ///
    /// ## Params
    ///
    /// * `lock_hash` - Cell lock script hash
    ///
    /// ## Examples
    ///
    /// Request
    ///
    /// ```json
    /// {
    ///   "id": 42,
    ///   "jsonrpc": "2.0",
    ///   "method": "deindex_lock_hash",
    ///   "params": [
    ///     "0x214ccd7362ec77349bc8df11e6edb54173338a3f6ec312e314849296f23aaec4"
    ///   ]
    /// }
    /// ```
    ///
    /// Response
    ///
    /// ```json
    /// {
    ///   "id": 42,
    ///   "jsonrpc": "2.0",
    ///   "result": null
    /// }
    /// ```
    #[rpc(name = "deprecated.deindex_lock_hash")]
    fn deindex_lock_hash(&self, lock_hash: H256) -> Result<()>;

    /// Returns states of all created lock hash indices.
    ///
    /// ## Examples
    ///
    /// Request
    ///
    /// ```json
    /// {
    ///   "id": 42,
    ///   "jsonrpc": "2.0",
    ///   "method": "get_lock_hash_index_states",
    ///   "params": []
    /// }
    /// ```
    ///
    /// Response
    ///
    /// ```json
    /// {
    ///   "id": 42,
    ///   "jsonrpc": "2.0",
    ///   "result": [
    ///     {
    ///       "block_hash": "0xa5f5c85987a15de25661e5a214f2c1449cd803f071acc7999820f25246471f40",
    ///       "block_number": "0x400",
    ///       "lock_hash": "0x4ceaa32f692948413e213ce6f3a83337145bde6e11fd8cb94377ce2637dcc412"
    ///     }
    ///   ]
    /// }
    /// ```
    #[rpc(name = "deprecated.get_lock_hash_index_states")]
    fn get_lock_hash_index_states(&self) -> Result<Vec<LockHashIndexState>>;

    /// Returns the total capacity by the hash of lock script.
    ///
    /// This RPC requires [creating the index](#tymethod.index_lock_hash) on `lock_hash` first.
    /// It returns the correct balance only if the index is created starting from the genesis block.
    ///
    /// ## Params
    ///
    /// * `lock_hash` - Cell lock script hash
    ///
    /// ## Examples
    ///
    /// Request
    ///
    /// ```json
    /// {
    ///   "id": 42,
    ///   "jsonrpc": "2.0",
    ///   "method": "get_capacity_by_lock_hash",
    ///   "params": [
    ///     "0x4ceaa32f692948413e213ce6f3a83337145bde6e11fd8cb94377ce2637dcc412"
    ///   ]
    /// }
    /// ```
    ///
    /// Response
    ///
    /// ```json
    /// {
    ///   "id": 42,
    ///   "jsonrpc": "2.0",
    ///   "result": {
    ///     "block_number": "0x400",
    ///     "capacity": "0xb00fb84df292",
    ///     "cells_count": "0x3f5"
    ///   }
    /// }
    /// ```
    #[rpc(name = "deprecated.get_capacity_by_lock_hash")]
    fn get_capacity_by_lock_hash(&self, lock_hash: H256) -> Result<Option<LockHashCapacity>>;
}

pub(crate) struct IndexerRpcImpl<WS> {
    pub store: WS,
}

impl<WS: IndexerStore + 'static> IndexerRpc for IndexerRpcImpl<WS> {
    fn get_live_cells_by_lock_hash(
        &self,
        lock_hash: H256,
        page: Uint64,
        per_page: Uint64,
        reverse_order: Option<bool>,
    ) -> Result<Vec<LiveCell>> {
        let lock_hash = lock_hash.pack();
        let per_page = (per_page.value() as usize).min(50);
        Ok(self
            .store
            .get_live_cells(
                &lock_hash,
                (page.value() as usize).saturating_mul(per_page),
                per_page,
                reverse_order.unwrap_or_default(),
            )
            .into_iter()
            .map(Into::into)
            .collect())
    }

    fn get_transactions_by_lock_hash(
        &self,
        lock_hash: H256,
        page: Uint64,
        per_page: Uint64,
        reverse_order: Option<bool>,
    ) -> Result<Vec<CellTransaction>> {
        let lock_hash = lock_hash.pack();
        let per_page = (per_page.value() as usize).min(50);
        Ok(self
            .store
            .get_transactions(
                &lock_hash,
                (page.value() as usize).saturating_mul(per_page),
                per_page,
                reverse_order.unwrap_or_default(),
            )
            .into_iter()
            .map(Into::into)
            .collect())
    }

    fn index_lock_hash(
        &self,
        lock_hash: H256,
        index_from: Option<BlockNumber>,
    ) -> Result<LockHashIndexState> {
        let state = self
            .store
            .insert_lock_hash(&lock_hash.pack(), index_from.map(Into::into));
        Ok(LockHashIndexState {
            lock_hash,
            block_number: state.block_number.into(),
            block_hash: state.block_hash.unpack(),
        })
    }

    fn deindex_lock_hash(&self, lock_hash: H256) -> Result<()> {
        self.store.remove_lock_hash(&lock_hash.pack());
        Ok(())
    }

    fn get_lock_hash_index_states(&self) -> Result<Vec<LockHashIndexState>> {
        let states = self
            .store
            .get_lock_hash_index_states()
            .into_iter()
            .map(|(lock_hash, state)| LockHashIndexState {
                lock_hash: lock_hash.unpack(),
                block_number: state.block_number.into(),
                block_hash: state.block_hash.unpack(),
            })
            .collect();
        Ok(states)
    }

    fn get_capacity_by_lock_hash(&self, lock_hash: H256) -> Result<Option<LockHashCapacity>> {
        let lock_hash = lock_hash.pack();
        Ok(self.store.get_capacity(&lock_hash).map(Into::into))
    }
}
