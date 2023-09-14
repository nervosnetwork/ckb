use crate::error::RPCError;
use crate::util::FeeRateCollector;
use ckb_jsonrpc_types::{
    BlockEconomicState, BlockFilter, BlockNumber, BlockResponse, BlockView, CellWithStatus,
    Consensus, EpochNumber, EpochView, EstimateCycles, FeeRateStatistics, HeaderView, OutPoint,
    ResponseFormat, ResponseFormatInnerType, Timestamp, Transaction, TransactionAndWitnessProof,
    TransactionProof, TransactionWithStatusResponse, Uint32, Uint64,
};
use ckb_logger::error;
use ckb_reward_calculator::RewardCalculator;
use ckb_shared::{shared::Shared, Snapshot};
use ckb_store::{data_loader_wrapper::AsDataLoader, ChainStore};
use ckb_traits::HeaderFieldsProvider;
use ckb_types::core::tx_pool::TransactionWithStatus;
use ckb_types::{
    core::{
        self,
        cell::{resolve_transaction, CellProvider, CellStatus, HeaderChecker},
        error::OutPointError,
    },
    packed,
    prelude::*,
    utilities::{merkle_root, MerkleProof, CBMT},
    H256,
};
use ckb_verification::ScriptVerifier;
use ckb_verification::TxVerifyEnv;
use jsonrpc_core::Result;
use jsonrpc_derive::rpc;
use std::collections::HashSet;
use std::sync::Arc;

/// RPC Module Chain for methods related to the canonical chain.
///
/// This module queries information about the canonical chain.
///
/// ## Canonical Chain
///
/// A canonical chain is the one with the most accumulated work. The accumulated work is
/// the sum of difficulties of all the blocks in the chain.
///
/// ## Chain Reorganization
///
/// Chain Reorganization happens when CKB found a chain that has accumulated more work than the
/// canonical chain. The reorganization reverts the blocks in the current canonical chain if needed,
/// and switch the canonical chain to that better chain.
///
/// ## Live Cell
///
/// A cell is live if
///
/// * it is found as an output in any transaction in the [canonical chain](#canonical-chain),
/// and
/// * it is not found as an input in any transaction in the canonical chain.
#[rpc(server)]
pub trait ChainRpc {
    /// Returns the information about a block by hash.
    ///
    /// ## Params
    ///
    /// * `block_hash` - the block hash.
    /// * `verbosity` - result format which allows 0 and 2. (**Optional**, the default is 2.)
    /// * `with_cycles` - whether the return cycles of block transactions. (**Optional**, default false.)
    ///
    /// ## Returns
    ///
    /// The RPC returns a block or null. When the RPC returns a block, the block hash must equal to
    /// the parameter `block_hash`.
    ///
    /// If the block is in the [canonical chain](#canonical-chain), the RPC must return the block
    /// information. Otherwise, the behavior is undefined. The RPC may return blocks found in local
    /// storage or simply returns null for all blocks that are not in the canonical chain. And
    /// because of [chain reorganization](#chain-reorganization), for the same `block_hash`, the
    /// RPC may sometimes return null and sometimes return the block.
    ///
    /// When `verbosity` is 2, it returns a JSON object as the `result`. See `BlockView` for the
    /// schema.
    ///
    /// When `verbosity` is 0, it returns a 0x-prefixed hex string as the `result`. The string
    /// encodes the block serialized by molecule using schema `table Block`.
    ///
    /// ## Examples
    ///
    /// Request
    ///
    /// ```json
    /// {
    ///   "id": 42,
    ///   "jsonrpc": "2.0",
    ///   "method": "get_block",
    ///   "params": [
    ///      "0xa5f5c85987a15de25661e5a214f2c1449cd803f071acc7999820f25246471f40"
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
    ///     "header": {
    ///       "compact_target": "0x1e083126",
    ///       "dao": "0xb5a3e047474401001bc476b9ee573000c0c387962a38000000febffacf030000",
    ///       "epoch": "0x7080018000001",
    ///       "extra_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
    ///       "hash": "0xa5f5c85987a15de25661e5a214f2c1449cd803f071acc7999820f25246471f40",
    ///       "nonce": "0x0",
    ///       "number": "0x400",
    ///       "parent_hash": "0xae003585fa15309b30b31aed3dcf385e9472c3c3e93746a6c4540629a6a1ed2d",
    ///       "proposals_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
    ///       "timestamp": "0x5cd2b117",
    ///       "transactions_root": "0xc47d5b78b3c4c4c853e2a32810818940d0ee403423bea9ec7b8e566d9595206c",
    ///       "version": "0x0"
    ///     },
    ///     "proposals": [],
    ///     "transactions": [
    ///       {
    ///         "cell_deps": [],
    ///         "hash": "0x365698b50ca0da75dca2c87f9e7b563811d3b5813736b8cc62cc3b106faceb17",
    ///         "header_deps": [],
    ///         "inputs": [
    ///           {
    ///             "previous_output": {
    ///               "index": "0xffffffff",
    ///               "tx_hash": "0x0000000000000000000000000000000000000000000000000000000000000000"
    ///             },
    ///             "since": "0x400"
    ///           }
    ///         ],
    ///         "outputs": [
    ///           {
    ///             "capacity": "0x18e64b61cf",
    ///             "lock": {
    ///               "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
    ///               "hash_type": "data",
    ///               "args": "0x"
    ///             },
    ///             "type": null
    ///           }
    ///         ],
    ///         "outputs_data": [
    ///           "0x"
    ///         ],
    ///         "version": "0x0",
    ///         "witnesses": [
    ///           "0x450000000c000000410000003500000010000000300000003100000028e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5000000000000000000"
    ///         ]
    ///       }
    ///     ],
    ///     "uncles": []
    ///   }
    /// }
    /// ```
    ///
    /// The response looks like below when `verbosity` is 0.
    ///
    /// ```text
    /// {
    ///   "id": 42,
    ///   "jsonrpc": "2.0",
    ///   "result": "0x..."
    /// }
    /// ```
    ///
    /// When specifying with_cycles, the response object will be different like below:
    ///
    /// ```text
    /// {
    ///     "id": 42,
    ///     "jsonrpc": "2.0",
    ///     "result": {
    ///         "block": <Object> or "0x...",
    ///         "cycles": []
    ///     }
    /// }
    /// ```
    #[rpc(name = "get_block")]
    fn get_block(
        &self,
        block_hash: H256,
        verbosity: Option<Uint32>,
        with_cycles: Option<bool>,
    ) -> Result<Option<BlockResponse>>;

    /// Returns the block in the [canonical chain](#canonical-chain) with the specific block number.
    ///
    /// ## Params
    ///
    /// * `block_number` - the block number.
    /// * `verbosity` - result format which allows 0 and 2. (**Optional**, the default is 2.)
    /// * `with_cycles` - whether the return cycles of block transactions. (**Optional**, default false.)
    ///
    /// ## Returns
    ///
    /// The RPC returns the block when `block_number` is less than or equal to the tip block
    /// number returned by [`get_tip_block_number`](#tymethod.get_tip_block_number) and returns
    /// null otherwise.
    ///
    /// Because of [chain reorganization](#chain-reorganization), the PRC may return null or even
    /// different blocks in different invocations with the same `block_number`.
    ///
    /// When `verbosity` is 2, it returns a JSON object as the `result`. See `BlockView` for the
    /// schema.
    ///
    /// When `verbosity` is 0, it returns a 0x-prefixed hex string as the `result`. The string
    /// encodes the block serialized by molecule using schema `table Block`.
    ///
    /// ## Errors
    ///
    /// * [`ChainIndexIsInconsistent (-201)`](../enum.RPCError.html#variant.ChainIndexIsInconsistent) - The index is inconsistent. It says a block hash is in the main chain, but cannot read it from the database.
    /// * [`DatabaseIsCorrupt (-202)`](../enum.RPCError.html#variant.DatabaseIsCorrupt) - The data read from database is dirty. Please report it as a bug.
    ///
    /// ## Examples
    ///
    /// Request
    ///
    /// ```json
    /// {
    ///   "id": 42,
    ///   "jsonrpc": "2.0",
    ///   "method": "get_block_by_number",
    ///   "params": [
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
    ///     "header": {
    ///       "compact_target": "0x1e083126",
    ///       "dao": "0xb5a3e047474401001bc476b9ee573000c0c387962a38000000febffacf030000",
    ///       "epoch": "0x7080018000001",
    ///       "extra_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
    ///       "hash": "0xa5f5c85987a15de25661e5a214f2c1449cd803f071acc7999820f25246471f40",
    ///       "nonce": "0x0",
    ///       "number": "0x400",
    ///       "parent_hash": "0xae003585fa15309b30b31aed3dcf385e9472c3c3e93746a6c4540629a6a1ed2d",
    ///       "proposals_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
    ///       "timestamp": "0x5cd2b117",
    ///       "transactions_root": "0xc47d5b78b3c4c4c853e2a32810818940d0ee403423bea9ec7b8e566d9595206c",
    ///       "version": "0x0"
    ///     },
    ///     "proposals": [],
    ///     "transactions": [
    ///       {
    ///         "cell_deps": [],
    ///         "hash": "0x365698b50ca0da75dca2c87f9e7b563811d3b5813736b8cc62cc3b106faceb17",
    ///         "header_deps": [],
    ///         "inputs": [
    ///           {
    ///             "previous_output": {
    ///               "index": "0xffffffff",
    ///               "tx_hash": "0x0000000000000000000000000000000000000000000000000000000000000000"
    ///             },
    ///             "since": "0x400"
    ///           }
    ///         ],
    ///         "outputs": [
    ///           {
    ///             "capacity": "0x18e64b61cf",
    ///             "lock": {
    ///               "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
    ///               "hash_type": "data",
    ///               "args": "0x"
    ///             },
    ///             "type": null
    ///           }
    ///         ],
    ///         "outputs_data": [
    ///           "0x"
    ///         ],
    ///         "version": "0x0",
    ///         "witnesses": [
    ///           "0x450000000c000000410000003500000010000000300000003100000028e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5000000000000000000"
    ///         ]
    ///       }
    ///     ],
    ///     "uncles": []
    ///   }
    /// }
    /// ```
    ///
    /// The response looks like below when `verbosity` is 0.
    ///
    /// ```text
    /// {
    ///   "id": 42,
    ///   "jsonrpc": "2.0",
    ///   "result": "0x..."
    /// }
    /// ```
    ///
    /// When specifying with_cycles, the response object will be different like below:
    ///
    /// ```text
    /// {
    ///     "id": 42,
    ///     "jsonrpc": "2.0",
    ///     "result": {
    ///         "block": <Object> or "0x...",
    ///         "cycles": []
    ///     }
    /// }
    /// ```
    #[rpc(name = "get_block_by_number")]
    fn get_block_by_number(
        &self,
        block_number: BlockNumber,
        verbosity: Option<Uint32>,
        with_cycles: Option<bool>,
    ) -> Result<Option<BlockResponse>>;

    /// Returns the information about a block header by hash.
    ///
    /// ## Params
    ///
    /// * `block_hash` - the block hash.
    /// * `verbosity` - result format which allows 0 and 1. (**Optional**, the default is 1.)
    ///
    /// ## Returns
    ///
    /// The RPC returns a header or null. When the RPC returns a header, the block hash must equal to
    /// the parameter `block_hash`.
    ///
    /// If the block is in the [canonical chain](#canonical-chain), the RPC must return the header
    /// information. Otherwise, the behavior is undefined. The RPC may return blocks found in local
    /// storage or simply returns null for all blocks that are not in the canonical chain. And
    /// because of [chain reorganization](#chain-reorganization), for the same `block_hash`, the
    /// RPC may sometimes return null and sometimes return the block header.
    ///
    /// When `verbosity` is 1, it returns a JSON object as the `result`. See `HeaderView` for the
    /// schema.
    ///
    /// When `verbosity` is 0, it returns a 0x-prefixed hex string as the `result`. The string
    /// encodes the block header serialized by molecule using schema `table Header`.
    ///
    /// ## Examples
    ///
    /// Request
    ///
    /// ```json
    /// {
    ///   "id": 42,
    ///   "jsonrpc": "2.0",
    ///   "method": "get_header",
    ///   "params": [
    ///     "0xa5f5c85987a15de25661e5a214f2c1449cd803f071acc7999820f25246471f40"
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
    ///     "compact_target": "0x1e083126",
    ///     "dao": "0xb5a3e047474401001bc476b9ee573000c0c387962a38000000febffacf030000",
    ///     "epoch": "0x7080018000001",
    ///     "extra_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
    ///     "hash": "0xa5f5c85987a15de25661e5a214f2c1449cd803f071acc7999820f25246471f40",
    ///     "nonce": "0x0",
    ///     "number": "0x400",
    ///     "parent_hash": "0xae003585fa15309b30b31aed3dcf385e9472c3c3e93746a6c4540629a6a1ed2d",
    ///     "proposals_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
    ///     "timestamp": "0x5cd2b117",
    ///     "transactions_root": "0xc47d5b78b3c4c4c853e2a32810818940d0ee403423bea9ec7b8e566d9595206c",
    ///     "version": "0x0"
    ///   }
    /// }
    /// ```
    ///
    /// The response looks like below when `verbosity` is 0.
    ///
    /// ```text
    /// {
    ///   "id": 42,
    ///   "jsonrpc": "2.0",
    ///   "result": "0x..."
    /// }
    /// ```
    #[rpc(name = "get_header")]
    fn get_header(
        &self,
        block_hash: H256,
        verbosity: Option<Uint32>,
    ) -> Result<Option<ResponseFormat<HeaderView>>>;

    /// Returns the block header in the [canonical chain](#canonical-chain) with the specific block
    /// number.
    ///
    /// ## Params
    ///
    /// * `block_number` - Number of a block
    /// * `verbosity` - result format which allows 0 and 1. (**Optional**, the default is 1.)
    ///
    /// ## Returns
    ///
    /// The RPC returns the block header when `block_number` is less than or equal to the tip block
    /// number returned by [`get_tip_block_number`](#tymethod.get_tip_block_number) and returns
    /// null otherwise.
    ///
    /// Because of [chain reorganization](#chain-reorganization), the PRC may return null or even
    /// different block headers in different invocations with the same `block_number`.
    ///
    /// When `verbosity` is 1, it returns a JSON object as the `result`. See `HeaderView` for the
    /// schema.
    ///
    /// When `verbosity` is 0, it returns a 0x-prefixed hex string as the `result`. The string
    /// encodes the block header serialized by molecule using schema `table Header`.
    ///
    /// ## Errors
    ///
    /// * [`ChainIndexIsInconsistent (-201)`](../enum.RPCError.html#variant.ChainIndexIsInconsistent) - The index is inconsistent. It says a block hash is in the main chain, but cannot read it from the database.
    ///
    /// ## Examples
    ///
    /// Request
    ///
    /// ```json
    /// {
    ///   "id": 42,
    ///   "jsonrpc": "2.0",
    ///   "method": "get_header_by_number",
    ///   "params": [
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
    ///     "compact_target": "0x1e083126",
    ///     "dao": "0xb5a3e047474401001bc476b9ee573000c0c387962a38000000febffacf030000",
    ///     "epoch": "0x7080018000001",
    ///     "extra_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
    ///     "hash": "0xa5f5c85987a15de25661e5a214f2c1449cd803f071acc7999820f25246471f40",
    ///     "nonce": "0x0",
    ///     "number": "0x400",
    ///     "parent_hash": "0xae003585fa15309b30b31aed3dcf385e9472c3c3e93746a6c4540629a6a1ed2d",
    ///     "proposals_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
    ///     "timestamp": "0x5cd2b117",
    ///     "transactions_root": "0xc47d5b78b3c4c4c853e2a32810818940d0ee403423bea9ec7b8e566d9595206c",
    ///     "version": "0x0"
    ///   }
    /// }
    /// ```
    ///
    /// The response looks like below when `verbosity` is 0.
    ///
    /// ```text
    /// {
    ///   "id": 42,
    ///   "jsonrpc": "2.0",
    ///   "result": "0x..."
    /// }
    /// ```
    #[rpc(name = "get_header_by_number")]
    fn get_header_by_number(
        &self,
        block_number: BlockNumber,
        verbosity: Option<Uint32>,
    ) -> Result<Option<ResponseFormat<HeaderView>>>;

    /// Returns the block filter by block hash.
    ///
    /// ## Params
    ///
    /// * `block_hash` - the block hash.
    ///
    /// ## Returns
    ///
    /// The block filter data
    ///
    /// ## Examples
    ///
    /// Request
    ///
    /// ```json
    /// {
    ///   "id": 42,
    ///   "jsonrpc": "2.0",
    ///   "method": "get_block_filter",
    ///   "params": [
    ///     "0xa5f5c85987a15de25661e5a214f2c1449cd803f071acc7999820f25246471f40"
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
    ///
    /// The response looks like below when the block have block filter.
    ///
    /// ```text
    /// {
    ///   "id": 42,
    ///   "jsonrpc": "2.0",
    ///   "result": {
    ///    "data": "0x...",
    ///    "hash": "0x..."
    ///   }
    /// }
    /// ```
    #[rpc(name = "get_block_filter")]
    fn get_block_filter(&self, block_hash: H256) -> Result<Option<BlockFilter>>;

    /// Returns the information about a transaction requested by transaction hash.
    ///
    /// ## Returns
    ///
    /// This RPC returns `null` if the transaction is not committed in the
    /// [canonical chain](#canonical-chain) nor the transaction memory pool.
    ///
    /// If the transaction is in the chain, the block hash is also returned.
    ///
    /// ## Params
    ///
    /// * `tx_hash` - Hash of a transaction
    /// * `verbosity` - result format which allows 0, 1 and 2. (**Optional**, the defaults to 2.)
    /// * `only_committed` - whether to query committed transaction only. (**Optional**, if not set, it will query all status of transactions.)
    ///
    /// ## Returns
    ///
    /// When verbosity=0, it's response value is as same as verbosity=2, but it
    /// return a 0x-prefixed hex encoded molecule packed::Transaction on `transaction` field
    ///
    /// When verbosity is 1: The RPC does not return the transaction content and the field transaction must be null.
    ///
    /// When verbosity is 2: if tx_status.status is pending, proposed, or committed,
    /// the RPC returns the transaction content as field transaction, otherwise the field is null.
    ///
    /// ## Examples
    ///
    /// Request
    ///
    /// ```json
    /// {
    ///   "id": 42,
    ///   "jsonrpc": "2.0",
    ///   "method": "get_transaction",
    ///   "params": [
    ///     "0xa0ef4eb5f4ceeb08a4c8524d84c5da95dce2f608e0ca2ec8091191b0f330c6e3"
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
    ///     "transaction": {
    ///       "cell_deps": [
    ///         {
    ///           "dep_type": "code",
    ///           "out_point": {
    ///             "index": "0x0",
    ///             "tx_hash": "0xa4037a893eb48e18ed4ef61034ce26eba9c585f15c9cee102ae58505565eccc3"
    ///           }
    ///         }
    ///       ],
    ///       "hash": "0xa0ef4eb5f4ceeb08a4c8524d84c5da95dce2f608e0ca2ec8091191b0f330c6e3",
    ///       "header_deps": [
    ///         "0x7978ec7ce5b507cfb52e149e36b1a23f6062ed150503c85bbf825da3599095ed"
    ///       ],
    ///       "inputs": [
    ///         {
    ///           "previous_output": {
    ///             "index": "0x0",
    ///             "tx_hash": "0x365698b50ca0da75dca2c87f9e7b563811d3b5813736b8cc62cc3b106faceb17"
    ///           },
    ///           "since": "0x0"
    ///         }
    ///       ],
    ///       "outputs": [
    ///         {
    ///           "capacity": "0x2540be400",
    ///           "lock": {
    ///             "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
    ///             "hash_type": "data",
    ///             "args": "0x"
    ///           },
    ///           "type": null
    ///         }
    ///       ],
    ///       "outputs_data": [
    ///         "0x"
    ///       ],
    ///       "version": "0x0",
    ///       "witnesses": []
    ///     },
    ///     "cycles": "0x219",
    ///     "time_added_to_pool" : "0x187b3d137a1",
    ///     "tx_status": {
    ///       "block_hash": null,
    ///       "status": "pending",
    ///       "reason": null
    ///     }
    ///   }
    /// }
    /// ```
    ///
    ///
    /// The response looks like below when `verbosity` is 0.
    ///
    /// ```text
    /// {
    ///   "id": 42,
    ///   "jsonrpc": "2.0",
    ///   "result": {
    ///     "transaction": "0x.....",
    ///     "cycles": "0x219",
    ///     "tx_status": {
    ///       "block_hash": null,
    ///       "status": "pending",
    ///       "reason": null
    ///     }
    ///   }
    /// }
    /// ```
    ///
    #[rpc(name = "get_transaction")]
    fn get_transaction(
        &self,
        tx_hash: H256,
        verbosity: Option<Uint32>,
        only_committed: Option<bool>,
    ) -> Result<TransactionWithStatusResponse>;

    /// Returns the hash of a block in the [canonical chain](#canonical-chain) with the specified
    /// `block_number`.
    ///
    /// ## Params
    ///
    /// * `block_number` - Block number
    ///
    /// ## Returns
    ///
    /// The RPC returns the block hash when `block_number` is less than or equal to the tip block
    /// number returned by [`get_tip_block_number`](#tymethod.get_tip_block_number) and returns
    /// null otherwise.
    ///
    /// Because of [chain reorganization](#chain-reorganization), the PRC may return null or even
    /// different block hashes in different invocations with the same `block_number`.
    ///
    /// ## Examples
    ///
    /// Request
    ///
    /// ```json
    /// {
    ///   "id": 42,
    ///   "jsonrpc": "2.0",
    ///   "method": "get_block_hash",
    ///   "params": [
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
    ///   "result": "0xa5f5c85987a15de25661e5a214f2c1449cd803f071acc7999820f25246471f40"
    /// }
    /// ```
    #[rpc(name = "get_block_hash")]
    fn get_block_hash(&self, block_number: BlockNumber) -> Result<Option<H256>>;

    /// Returns the header with the highest block number in the [canonical chain](#canonical-chain).
    ///
    /// Because of [chain reorganization](#chain-reorganization), the block number returned can be
    /// less than previous invocations and different invocations may return different block headers
    /// with the same block number.
    ///
    /// ## Params
    ///
    /// * `verbosity` - result format which allows 0 and 1. (**Optional**, the default is 1.)
    ///
    /// ## Returns
    ///
    /// When `verbosity` is 1, the RPC returns a JSON object as the `result`. See HeaderView for the
    /// schema.
    ///
    /// When `verbosity` is 0, it returns a 0x-prefixed hex string as the `result`. The string
    /// encodes the header serialized by molecule using schema `table Header`.
    ///
    /// ## Examples
    ///
    /// Request
    ///
    /// ```json
    /// {
    ///   "id": 42,
    ///   "jsonrpc": "2.0",
    ///   "method": "get_tip_header",
    ///   "params": []
    /// }
    /// ```
    ///
    /// Response
    ///
    /// ```json
    /// {
    ///   "jsonrpc": "2.0",
    ///   "result": {
    ///     "compact_target": "0x1e083126",
    ///     "dao": "0xb5a3e047474401001bc476b9ee573000c0c387962a38000000febffacf030000",
    ///     "epoch": "0x7080018000001",
    ///     "extra_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
    ///     "hash": "0xa5f5c85987a15de25661e5a214f2c1449cd803f071acc7999820f25246471f40",
    ///     "nonce": "0x0",
    ///     "number": "0x400",
    ///     "parent_hash": "0xae003585fa15309b30b31aed3dcf385e9472c3c3e93746a6c4540629a6a1ed2d",
    ///     "proposals_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
    ///     "timestamp": "0x5cd2b117",
    ///     "transactions_root": "0xc47d5b78b3c4c4c853e2a32810818940d0ee403423bea9ec7b8e566d9595206c",
    ///     "version": "0x0"
    ///   },
    ///   "id": 42
    /// }
    /// ```
    ///
    /// The response looks like below when `verbosity` is 0.
    ///
    /// ```text
    /// {
    ///   "id": 42,
    ///   "jsonrpc": "2.0",
    ///   "result": "0x..."
    /// }
    /// ```
    #[rpc(name = "get_tip_header")]
    fn get_tip_header(&self, verbosity: Option<Uint32>) -> Result<ResponseFormat<HeaderView>>;

    /// Returns the status of a cell. The RPC returns extra information if it is a [live cell](#live-cell).
    ///
    /// ## Returns
    ///
    /// This RPC tells whether a cell is live or not.
    ///
    /// If the cell is live, the RPC will return details about the cell. Otherwise, the field `cell` is
    /// null in the result.
    ///
    /// If the cell is live and `with_data` is set to `false`, the field `cell.data` is null in the
    /// result.
    ///
    /// ## Params
    ///
    /// * `out_point` - Reference to the cell by transaction hash and output index.
    /// * `with_data` - Whether the RPC should return cell data. Cell data can be huge, if the client
    /// does not need the data, it should set this to `false` to save bandwidth.
    ///
    /// ## Examples
    ///
    /// Request
    ///
    /// ```json
    /// {
    ///   "id": 42,
    ///   "jsonrpc": "2.0",
    ///   "method": "get_live_cell",
    ///   "params": [
    ///     {
    ///       "index": "0x0",
    ///       "tx_hash": "0xa4037a893eb48e18ed4ef61034ce26eba9c585f15c9cee102ae58505565eccc3"
    ///     },
    ///     true
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
    ///     "cell": {
    ///       "data": {
    ///         "content": "0x7f454c460201010000000000000000000200f3000100000078000100000000004000000000000000980000000000000005000000400038000100400003000200010000000500000000000000000000000000010000000000000001000000000082000000000000008200000000000000001000000000000001459308d00573000000002e7368737472746162002e74657874000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000b000000010000000600000000000000780001000000000078000000000000000a0000000000000000000000000000000200000000000000000000000000000001000000030000000000000000000000000000000000000082000000000000001100000000000000000000000000000001000000000000000000000000000000",
    ///         "hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5"
    ///       },
    ///       "output": {
    ///         "capacity": "0x802665800",
    ///         "lock": {
    ///           "code_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
    ///           "hash_type": "data",
    ///           "args": "0x"
    ///         },
    ///         "type": null
    ///       }
    ///     },
    ///     "status": "live"
    ///   }
    /// }
    /// ```
    #[rpc(name = "get_live_cell")]
    fn get_live_cell(&self, out_point: OutPoint, with_data: bool) -> Result<CellWithStatus>;

    /// Returns the highest block number in the [canonical chain](#canonical-chain).
    ///
    /// Because of [chain reorganization](#chain-reorganization), the returned block number may be
    /// less than a value returned in the previous invocation.
    ///
    /// ## Examples
    ///
    /// Request
    ///
    /// ```json
    /// {
    ///   "id": 42,
    ///   "jsonrpc": "2.0",
    ///   "method": "get_tip_block_number",
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
    ///   "result": "0x400"
    /// }
    /// ```
    #[rpc(name = "get_tip_block_number")]
    fn get_tip_block_number(&self) -> Result<BlockNumber>;

    /// Returns the epoch with the highest number in the [canonical chain](#canonical-chain).
    ///
    /// Pay attention that like blocks with the specific block number may change because of [chain
    /// reorganization](#chain-reorganization), This RPC may return different epochs which have
    /// the same epoch number.
    ///
    /// ## Examples
    ///
    /// Request
    ///
    /// ```json
    /// {
    ///   "id": 42,
    ///   "jsonrpc": "2.0",
    ///   "method": "get_current_epoch",
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
    ///   "result": {
    ///     "compact_target": "0x1e083126",
    ///     "length": "0x708",
    ///     "number": "0x1",
    ///     "start_number": "0x3e8"
    ///   }
    /// }
    /// ```
    #[rpc(name = "get_current_epoch")]
    fn get_current_epoch(&self) -> Result<EpochView>;

    /// Returns the epoch in the [canonical chain](#canonical-chain) with the specific epoch number.
    ///
    /// ## Params
    ///
    /// * `epoch_number` - Epoch number
    ///
    /// ## Returns
    ///
    /// The RPC returns the epoch when `epoch_number` is less than or equal to the current epoch number
    /// returned by [`get_current_epoch`](#tymethod.get_current_epoch) and returns null otherwise.
    ///
    /// Because of [chain reorganization](#chain-reorganization), for the same `epoch_number`, this
    /// RPC may return null or different epochs in different invocations.
    ///
    /// ## Examples
    ///
    /// Request
    ///
    /// ```json
    /// {
    ///   "id": 42,
    ///   "jsonrpc": "2.0",
    ///   "method": "get_epoch_by_number",
    ///   "params": [
    ///     "0x0"
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
    ///     "compact_target": "0x20010000",
    ///     "length": "0x3e8",
    ///     "number": "0x0",
    ///     "start_number": "0x0"
    ///   }
    /// }
    /// ```
    #[rpc(name = "get_epoch_by_number")]
    fn get_epoch_by_number(&self, epoch_number: EpochNumber) -> Result<Option<EpochView>>;

    /// Returns increased issuance, miner reward, and the total transaction fee of a block.
    ///
    /// This RPC returns null if the block is not in the [canonical chain](#canonical-chain).
    ///
    /// CKB delays CKB creation for miners. The output cells in the cellbase of block N are for the
    /// miner creating block `N - 1 - ProposalWindow.farthest`.
    ///
    /// In mainnet, `ProposalWindow.farthest` is 10, so the outputs in block 100 are rewards for
    /// miner creating block 89.
    ///
    /// Because of the delay, this RPC returns null if the block rewards are not finalized yet. For
    /// example, the economic state for block 89 is only available when the number returned by
    /// [`get_tip_block_number`](#tymethod.get_tip_block_number) is greater than or equal to 100.
    ///
    /// ## Params
    ///
    /// * `block_hash` - Specifies the block hash which rewards should be analyzed.
    ///
    /// ## Returns
    ///
    /// If the block with the hash `block_hash` is in the [canonical chain](#canonical-chain) and
    /// its rewards have been finalized, return the block rewards analysis for this block. A special
    /// case is that the return value for genesis block is null.
    ///
    /// ## Examples
    ///
    /// Request
    ///
    /// ```json
    /// {
    ///   "id": 42,
    ///   "jsonrpc": "2.0",
    ///   "method": "get_block_economic_state",
    ///   "params": [
    ///     "0x02530b25ad0ff677acc365cb73de3e8cc09c7ddd58272e879252e199d08df83b"
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
    ///     "finalized_at": "0xa5f5c85987a15de25661e5a214f2c1449cd803f071acc7999820f25246471f40",
    ///     "issuance": {
    ///       "primary": "0x18ce922bca",
    ///       "secondary": "0x7f02ec655"
    ///     },
    ///     "miner_reward": {
    ///       "committed": "0x0",
    ///       "primary": "0x18ce922bca",
    ///       "proposal": "0x0",
    ///       "secondary": "0x17b93605"
    ///     },
    ///     "txs_fee": "0x0"
    ///   }
    /// }
    /// ```
    #[rpc(name = "get_block_economic_state")]
    fn get_block_economic_state(&self, block_hash: H256) -> Result<Option<BlockEconomicState>>;

    /// Returns a Merkle proof that transactions are included in a block.
    ///
    /// ## Params
    ///
    /// * `tx_hashes` - Transaction hashes, all transactions must be in the same block
    /// * `block_hash` - An optional parameter, if specified, looks for transactions in the block with this hash
    ///
    /// ## Examples
    ///
    /// Request
    ///
    /// ```json
    /// {
    ///   "id": 42,
    ///   "jsonrpc": "2.0",
    ///   "method": "get_transaction_proof",
    ///   "params": [
    ///     [ "0xa4037a893eb48e18ed4ef61034ce26eba9c585f15c9cee102ae58505565eccc3" ]
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
    ///     "block_hash": "0x7978ec7ce5b507cfb52e149e36b1a23f6062ed150503c85bbf825da3599095ed",
    ///     "proof": {
    ///       "indices": [ "0x0" ],
    ///       "lemmas": []
    ///     },
    ///     "witnesses_root": "0x2bb631f4a251ec39d943cc238fc1e39c7f0e99776e8a1e7be28a03c70c4f4853"
    ///   }
    /// }
    /// ```
    #[rpc(name = "get_transaction_proof")]
    fn get_transaction_proof(
        &self,
        tx_hashes: Vec<H256>,
        block_hash: Option<H256>,
    ) -> Result<TransactionProof>;

    /// Verifies that a proof points to transactions in a block, returning the transaction hashes it commits to.
    ///
    /// ## Parameters
    ///
    /// * `transaction_proof` - proof generated by [`get_transaction_proof`](#tymethod.get_transaction_proof).
    ///
    /// ## Examples
    ///
    /// Request
    ///
    /// ```json
    /// {
    ///   "id": 42,
    ///   "jsonrpc": "2.0",
    ///   "method": "verify_transaction_proof",
    ///   "params": [
    ///     {
    ///       "block_hash": "0x7978ec7ce5b507cfb52e149e36b1a23f6062ed150503c85bbf825da3599095ed",
    ///       "proof": {
    ///         "indices": [ "0x0" ],
    ///         "lemmas": []
    ///       },
    ///       "witnesses_root": "0x2bb631f4a251ec39d943cc238fc1e39c7f0e99776e8a1e7be28a03c70c4f4853"
    ///     }
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
    ///     "0xa4037a893eb48e18ed4ef61034ce26eba9c585f15c9cee102ae58505565eccc3"
    ///   ]
    /// }
    /// ```
    #[rpc(name = "verify_transaction_proof")]
    fn verify_transaction_proof(&self, tx_proof: TransactionProof) -> Result<Vec<H256>>;

    /// Returns a Merkle proof of transactions' witness included in a block.
    ///
    /// ## Params
    ///
    /// * `tx_hashes` - Transaction hashes, all transactions must be in the same block
    /// * `block_hash` - An optional parameter, if specified, looks for transactions in the block with this hash
    ///
    /// ## Examples
    ///
    /// Request
    ///
    /// ```json
    /// {
    ///   "id": 42,
    ///   "jsonrpc": "2.0",
    ///   "method": "get_transaction_and_witness_proof",
    ///   "params": [
    ///     [ "0xa4037a893eb48e18ed4ef61034ce26eba9c585f15c9cee102ae58505565eccc3" ]
    ///   ]
    /// }
    /// ```
    ///
    /// Response
    ///
    /// ```json
    /// {
    ///     "jsonrpc": "2.0",
    ///     "result": {
    ///         "block_hash": "0x7978ec7ce5b507cfb52e149e36b1a23f6062ed150503c85bbf825da3599095ed",
    ///         "transactions_proof": {
    ///             "indices": [ "0x0" ],
    ///             "lemmas": []
    ///         },
    ///         "witnesses_proof": {
    ///             "indices": [
    ///                 "0x0"
    ///             ],
    ///             "lemmas": []
    ///         }
    ///     },
    ///     "id": 42
    /// }
    /// ```
    #[rpc(name = "get_transaction_and_witness_proof")]
    fn get_transaction_and_witness_proof(
        &self,
        tx_hashes: Vec<H256>,
        block_hash: Option<H256>,
    ) -> Result<TransactionAndWitnessProof>;

    /// Verifies that a proof points to transactions in a block, returning the transaction hashes it commits to.
    ///
    /// ## Parameters
    ///
    /// * `tx_proof` - proof generated by [`get_transaction_and_witness_proof`](#tymethod.get_transaction_and_witness_proof).
    ///
    /// ## Examples
    ///
    /// Request
    ///
    /// ```json
    /// {
    ///   "id": 42,
    ///   "jsonrpc": "2.0",
    ///   "method": "verify_transaction_and_witness_proof",
    ///   "params": [
    ///     {
    ///       "block_hash": "0x7978ec7ce5b507cfb52e149e36b1a23f6062ed150503c85bbf825da3599095ed",
    ///         "transactions_proof": {
    ///             "indices": [ "0x0" ],
    ///             "lemmas": []
    ///         },
    ///         "witnesses_proof": {
    ///             "indices": [
    ///                 "0x0"
    ///             ],
    ///             "lemmas": []
    ///         }
    ///     }
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
    ///     "0xa4037a893eb48e18ed4ef61034ce26eba9c585f15c9cee102ae58505565eccc3"
    ///   ]
    /// }
    /// ```
    #[rpc(name = "verify_transaction_and_witness_proof")]
    fn verify_transaction_and_witness_proof(
        &self,
        tx_proof: TransactionAndWitnessProof,
    ) -> Result<Vec<H256>>;

    /// Returns the information about a fork block by hash.
    ///
    /// ## Params
    ///
    /// * `block_hash` - the fork block hash.
    /// * `verbosity` - result format which allows 0 and 2. (**Optional**, the default is 2.)
    ///
    /// ## Returns
    ///
    /// The RPC returns a fork block or null. When the RPC returns a block, the block hash must equal to
    /// the parameter `block_hash`.
    ///
    /// Please note that due to the technical nature of the peer to peer sync, the RPC may return null or a fork block
    /// result on different nodes with same `block_hash` even they are fully synced to the [canonical chain](#canonical-chain).
    /// And because of [chain reorganization](#chain-reorganization), for the same `block_hash`, the
    /// RPC may sometimes return null and sometimes return the fork block.
    ///
    /// When `verbosity` is 2, it returns a JSON object as the `result`. See `BlockView` for the
    /// schema.
    ///
    /// When `verbosity` is 0, it returns a 0x-prefixed hex string as the `result`. The string
    /// encodes the block serialized by molecule using schema `table Block`.
    ///
    /// ## Examples
    ///
    /// Request
    ///
    /// ```json
    /// {
    ///   "id": 42,
    ///   "jsonrpc": "2.0",
    ///   "method": "get_fork_block",
    ///   "params": [
    ///     "0xdca341a42890536551f99357612cef7148ed471e3b6419d0844a4e400be6ee94"
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
    ///     "header": {
    ///       "compact_target": "0x1e083126",
    ///       "dao": "0xb5a3e047474401001bc476b9ee573000c0c387962a38000000febffacf030000",
    ///       "epoch": "0x7080018000001",
    ///       "extra_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
    ///       "hash": "0xdca341a42890536551f99357612cef7148ed471e3b6419d0844a4e400be6ee94",
    ///       "nonce": "0x0",
    ///       "number": "0x400",
    ///       "parent_hash": "0xae003585fa15309b30b31aed3dcf385e9472c3c3e93746a6c4540629a6a1ed2d",
    ///       "proposals_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
    ///       "timestamp": "0x5cd2b118",
    ///       "transactions_root": "0xc47d5b78b3c4c4c853e2a32810818940d0ee403423bea9ec7b8e566d9595206c",
    ///       "version": "0x0"
    ///     },
    ///     "proposals": [],
    ///     "transactions": [
    ///       {
    ///         "cell_deps": [],
    ///         "hash": "0x365698b50ca0da75dca2c87f9e7b563811d3b5813736b8cc62cc3b106faceb17",
    ///         "header_deps": [],
    ///         "inputs": [
    ///           {
    ///             "previous_output": {
    ///               "index": "0xffffffff",
    ///               "tx_hash": "0x0000000000000000000000000000000000000000000000000000000000000000"
    ///             },
    ///             "since": "0x400"
    ///           }
    ///         ],
    ///         "outputs": [
    ///           {
    ///             "capacity": "0x18e64b61cf",
    ///             "lock": {
    ///               "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
    ///               "hash_type": "data",
    ///               "args": "0x"
    ///             },
    ///             "type": null
    ///           }
    ///         ],
    ///         "outputs_data": [
    ///           "0x"
    ///         ],
    ///         "version": "0x0",
    ///         "witnesses": [
    ///           "0x450000000c000000410000003500000010000000300000003100000028e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5000000000000000000"
    ///         ]
    ///       }
    ///     ],
    ///     "uncles": []
    ///   }
    /// }
    /// ```
    ///
    /// The response looks like below when `verbosity` is 0.
    ///
    /// ```text
    /// {
    ///   "id": 42,
    ///   "jsonrpc": "2.0",
    ///   "result": "0x..."
    /// }
    /// ```
    #[rpc(name = "get_fork_block")]
    fn get_fork_block(
        &self,
        block_hash: H256,
        verbosity: Option<Uint32>,
    ) -> Result<Option<ResponseFormat<BlockView>>>;

    /// Return various consensus parameters.
    ///
    /// ## Returns
    ///
    /// If any hardfork feature has `epoch=null`, it means the feature will never be activated.
    ///
    /// ## Examples
    ///
    /// Request
    ///
    /// ```json
    /// {
    ///   "id": 42,
    ///   "jsonrpc": "2.0",
    ///   "method": "get_consensus",
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
    ///   "result": {
    ///         "block_version": "0x0",
    ///         "cellbase_maturity": "0x10000000000",
    ///         "dao_type_hash": null,
    ///         "epoch_duration_target": "0x3840",
    ///         "genesis_hash": "0x7978ec7ce5b507cfb52e149e36b1a23f6062ed150503c85bbf825da3599095ed",
    ///         "hardfork_features": [
    ///             { "rfc": "0028", "epoch_number": "0x1526" },
    ///             { "rfc": "0029", "epoch_number": "0x0" },
    ///             { "rfc": "0030", "epoch_number": "0x0" },
    ///             { "rfc": "0031", "epoch_number": "0x0" },
    ///             { "rfc": "0032", "epoch_number": "0x0" },
    ///             { "rfc": "0036", "epoch_number": "0x0" },
    ///             { "rfc": "0038", "epoch_number": "0x0" },
    ///             { "rfc": "0048", "epoch_number": null },
    ///             { "rfc": "0049", "epoch_number": null }
    ///          ],
    ///         "id": "main",
    ///         "initial_primary_epoch_reward": "0x71afd498d000",
    ///         "max_block_bytes": "0x91c08",
    ///         "max_block_cycles": "0xd09dc300",
    ///         "max_block_proposals_limit": "0x5dc",
    ///         "max_uncles_num": "0x2",
    ///         "median_time_block_count": "0x25",
    ///         "orphan_rate_target": {
    ///             "denom": "0x28",
    ///             "numer": "0x1"
    ///         },
    ///         "permanent_difficulty_in_dummy": false,
    ///         "primary_epoch_reward_halving_interval": "0x2238",
    ///         "proposer_reward_ratio": {
    ///             "denom": "0xa",
    ///             "numer": "0x4"
    ///         },
    ///         "secondary_epoch_reward": "0x37d0c8e28542",
    ///         "secp256k1_blake160_multisig_all_type_hash": null,
    ///         "secp256k1_blake160_sighash_all_type_hash": null,
    ///         "softforks": {
    ///             "testdummy": {
    ///                 "status": "rfc0043",
    ///                 "rfc0043": {
    ///                     "bit": 1,
    ///                     "min_activation_epoch": "0x0",
    ///                     "period": "0xa",
    ///                     "start": "0x0",
    ///                     "threshold": {
    ///                         "denom": "0x4",
    ///                         "numer": "0x3"
    ///                     },
    ///                     "timeout": "0x0"
    ///                 }
    ///             }
    ///         },
    ///         "tx_proposal_window": {
    ///             "closest": "0x2",
    ///             "farthest": "0xa"
    ///         },
    ///         "tx_version": "0x0",
    ///         "type_id_code_hash": "0x00000000000000000000000000000000000000000000000000545950455f4944"
    ///     }
    /// }
    /// ```
    #[rpc(name = "get_consensus")]
    fn get_consensus(&self) -> Result<Consensus>;

    /// Returns the past median time by block hash.
    ///
    /// ## Params
    ///
    /// * `block_hash` - A median time is calculated for a consecutive block sequence. `block_hash` indicates the highest block of the sequence.
    ///
    /// ## Returns
    ///
    /// When the given block hash is not on the current canonical chain, this RPC returns null;
    /// otherwise returns the median time of the consecutive 37 blocks where the given block_hash has the highest height.
    ///
    /// Note that the given block is included in the median time. The included block number range is `[MAX(block - 36, 0), block]`.
    ///
    /// ## Examples
    ///
    /// Request
    ///
    /// ```json
    /// {
    ///   "id": 42,
    ///   "jsonrpc": "2.0",
    ///   "method": "get_block_median_time",
    ///   "params": [
    ///     "0xa5f5c85987a15de25661e5a214f2c1449cd803f071acc7999820f25246471f40"
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
    ///   "result": "0x5cd2b105"
    /// }
    /// ```
    #[rpc(name = "get_block_median_time")]
    fn get_block_median_time(&self, block_hash: H256) -> Result<Option<Timestamp>>;

    /// `estimate_cycles` run a transaction and return the execution consumed cycles.
    ///
    /// This method will not check the transaction validity, but only run the lock script
    /// and type script and then return the execution cycles.
    ///
    /// It is used to estimate how many cycles the scripts consume.
    ///
    /// ## Errors
    ///
    /// * [`TransactionFailedToResolve (-301)`](../enum.RPCError.html#variant.TransactionFailedToResolve) - Failed to resolve the referenced cells and headers used in the transaction, as inputs or dependencies.
    /// * [`TransactionFailedToVerify (-302)`](../enum.RPCError.html#variant.TransactionFailedToVerify) - There is a script returns with an error.
    ///
    /// ## Examples
    ///
    /// Request
    ///
    /// ```json
    /// {
    ///   "id": 42,
    ///   "jsonrpc": "2.0",
    ///   "method": "estimate_cycles",
    ///   "params": [
    ///     {
    ///       "cell_deps": [
    ///         {
    ///           "dep_type": "code",
    ///           "out_point": {
    ///             "index": "0x0",
    ///             "tx_hash": "0xa4037a893eb48e18ed4ef61034ce26eba9c585f15c9cee102ae58505565eccc3"
    ///           }
    ///         }
    ///       ],
    ///       "header_deps": [
    ///         "0x7978ec7ce5b507cfb52e149e36b1a23f6062ed150503c85bbf825da3599095ed"
    ///       ],
    ///       "inputs": [
    ///         {
    ///           "previous_output": {
    ///             "index": "0x0",
    ///             "tx_hash": "0x365698b50ca0da75dca2c87f9e7b563811d3b5813736b8cc62cc3b106faceb17"
    ///           },
    ///           "since": "0x0"
    ///         }
    ///       ],
    ///       "outputs": [
    ///         {
    ///           "capacity": "0x2540be400",
    ///           "lock": {
    ///             "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
    ///             "hash_type": "data",
    ///             "args": "0x"
    ///           },
    ///           "type": null
    ///         }
    ///       ],
    ///       "outputs_data": [
    ///         "0x"
    ///       ],
    ///       "version": "0x0",
    ///       "witnesses": []
    ///     }
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
    ///     "cycles": "0x219"
    ///   }
    /// }
    /// ```
    #[rpc(name = "estimate_cycles")]
    fn estimate_cycles(&self, tx: Transaction) -> Result<EstimateCycles>;

    /// Returns the fee_rate statistics of confirmed blocks on the chain
    ///
    /// ## Params
    ///
    /// * `target` - Specify the number (1 - 101) of confirmed blocks to be counted.
    ///  If the number is even, automatically add one. If not specified, defaults to 21
    ///
    /// ## Returns
    ///
    /// If the query finds the corresponding historical data,
    /// the corresponding statistics are returned,
    /// containing the mean and median, in shannons per kilo-weight.
    /// If not, it returns null.
    ///
    /// ## Examples
    ///
    /// Request
    ///
    /// ```json
    /// {
    ///   "id": 42,
    ///   "jsonrpc": "2.0",
    ///   "method": "get_fee_rate_statics",
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
    ///   "result": {
    ///     "mean": "0xe79d",
    ///     "median": "0x14a8"
    ///    }
    /// }
    /// ```
    #[deprecated(
        since = "0.109.0",
        note = "Please use the RPC method [`get_fee_rate_statistics`](#tymethod.get_fee_rate_statistics) instead"
    )]
    #[rpc(name = "get_fee_rate_statics")]
    fn get_fee_rate_statics(&self, target: Option<Uint64>) -> Result<Option<FeeRateStatistics>>;

    /// Returns the fee_rate statistics of confirmed blocks on the chain
    ///
    /// ## Params
    ///
    /// * `target` - Specify the number (1 - 101) of confirmed blocks to be counted.
    ///  If the number is even, automatically add one. If not specified, defaults to 21
    ///
    /// ## Returns
    ///
    /// If the query finds the corresponding historical data,
    /// the corresponding statistics are returned,
    /// containing the mean and median, in shannons per kilo-weight.
    /// If not, it returns null.
    ///
    /// ## Examples
    ///
    /// Request
    ///
    /// ```json
    /// {
    ///   "id": 42,
    ///   "jsonrpc": "2.0",
    ///   "method": "get_fee_rate_statistics",
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
    ///   "result": {
    ///     "mean": "0xe79d",
    ///     "median": "0x14a8"
    ///    }
    /// }
    /// ```
    #[rpc(name = "get_fee_rate_statistics")]
    fn get_fee_rate_statistics(&self, target: Option<Uint64>) -> Result<Option<FeeRateStatistics>>;
}

pub(crate) struct ChainRpcImpl {
    pub shared: Shared,
}

const DEFAULT_BLOCK_VERBOSITY_LEVEL: u32 = 2;
const DEFAULT_HEADER_VERBOSITY_LEVEL: u32 = 1;
const DEFAULT_GET_TRANSACTION_VERBOSITY_LEVEL: u32 = 2;

impl ChainRpc for ChainRpcImpl {
    fn get_block(
        &self,
        block_hash: H256,
        verbosity: Option<Uint32>,
        with_cycles: Option<bool>,
    ) -> Result<Option<BlockResponse>> {
        let snapshot = self.shared.snapshot();
        let block_hash = block_hash.pack();

        self.get_block_by_hash(&snapshot, &block_hash, verbosity, with_cycles)
    }

    fn get_block_by_number(
        &self,
        block_number: BlockNumber,
        verbosity: Option<Uint32>,
        with_cycles: Option<bool>,
    ) -> Result<Option<BlockResponse>> {
        let snapshot = self.shared.snapshot();
        let block_hash = match snapshot.get_block_hash(block_number.into()) {
            Some(block_hash) => block_hash,
            None => return Ok(None),
        };

        let ret = self.get_block_by_hash(&snapshot, &block_hash, verbosity, with_cycles);
        if ret == Ok(None) {
            let message = format!(
                "Chain Index says block #{block_number} is {block_hash:#x}, but that block is not in the database"
            );
            error!("{message}");
            return Err(RPCError::custom(
                RPCError::ChainIndexIsInconsistent,
                message,
            ));
        }
        ret
    }

    fn get_header(
        &self,
        block_hash: H256,
        verbosity: Option<Uint32>,
    ) -> Result<Option<ResponseFormat<HeaderView>>> {
        let snapshot = self.shared.snapshot();
        let block_hash = block_hash.pack();
        if !snapshot.is_main_chain(&block_hash) {
            return Ok(None);
        }

        let verbosity = verbosity
            .map(|v| v.value())
            .unwrap_or(DEFAULT_HEADER_VERBOSITY_LEVEL);
        if verbosity == 1 {
            Ok(snapshot
                .get_block_header(&block_hash)
                .map(|header| ResponseFormat::json(header.into())))
        } else if verbosity == 0 {
            Ok(snapshot
                .get_packed_block_header(&block_hash)
                .map(|packed| ResponseFormat::hex(packed.as_bytes())))
        } else {
            Err(RPCError::invalid_params("invalid verbosity level"))
        }
    }

    fn get_header_by_number(
        &self,
        block_number: BlockNumber,
        verbosity: Option<Uint32>,
    ) -> Result<Option<ResponseFormat<HeaderView>>> {
        let snapshot = self.shared.snapshot();
        let block_hash = match snapshot.get_block_hash(block_number.into()) {
            Some(block_hash) => block_hash,
            None => return Ok(None),
        };

        let verbosity = verbosity
            .map(|v| v.value())
            .unwrap_or(DEFAULT_HEADER_VERBOSITY_LEVEL);
        let result = if verbosity == 1 {
            snapshot
                .get_block_header(&block_hash)
                .map(|header| Some(ResponseFormat::json(header.into())))
        } else if verbosity == 0 {
            snapshot
                .get_packed_block_header(&block_hash)
                .map(|header| Some(ResponseFormat::hex(header.as_bytes())))
        } else {
            return Err(RPCError::invalid_params("invalid verbosity level"));
        };

        result.ok_or_else(|| {
            let message = format!(
                "Chain Index says block #{block_number} is {block_hash:#x}, but that block is not in the database"
            );
            error!("{message}");
            RPCError::custom(RPCError::ChainIndexIsInconsistent, message)
        })
    }

    fn get_block_filter(&self, block_hash: H256) -> Result<Option<BlockFilter>> {
        let store = self.shared.store();
        let block_hash = block_hash.pack();
        if !store.is_main_chain(&block_hash) {
            return Ok(None);
        }
        Ok(store.get_block_filter(&block_hash).map(|data| {
            let hash = store
                .get_block_filter_hash(&block_hash)
                .expect("stored filter hash");
            BlockFilter {
                data: data.into(),
                hash: hash.into(),
            }
        }))
    }

    fn get_transaction(
        &self,
        tx_hash: H256,
        verbosity: Option<Uint32>,
        only_committed: Option<bool>,
    ) -> Result<TransactionWithStatusResponse> {
        let tx_hash = tx_hash.pack();
        let verbosity = verbosity
            .map(|v| v.value())
            .unwrap_or(DEFAULT_GET_TRANSACTION_VERBOSITY_LEVEL);

        let only_committed: bool = only_committed.unwrap_or(false);

        if verbosity == 0 {
            // when verbosity=0, it's response value is as same as verbosity=2, but it
            // return a 0x-prefixed hex encoded molecule packed::Transaction` on `transaction` field
            self.get_transaction_verbosity2(tx_hash, only_committed)
                .map(|tws| TransactionWithStatusResponse::from(tws, ResponseFormatInnerType::Hex))
        } else if verbosity == 1 {
            // The RPC does not return the transaction content and the field transaction must be null.
            self.get_transaction_verbosity1(tx_hash, only_committed)
                .map(|tws| TransactionWithStatusResponse::from(tws, ResponseFormatInnerType::Json))
        } else if verbosity == 2 {
            // if tx_status.status is pending, proposed, or committed,
            // the RPC returns the transaction content as field transaction,
            // otherwise the field is null.
            self.get_transaction_verbosity2(tx_hash, only_committed)
                .map(|tws| TransactionWithStatusResponse::from(tws, ResponseFormatInnerType::Json))
        } else {
            Err(RPCError::invalid_params("invalid verbosity level"))
        }
    }

    fn get_block_hash(&self, block_number: BlockNumber) -> Result<Option<H256>> {
        Ok(self
            .shared
            .snapshot()
            .get_block_hash(block_number.into())
            .map(|h| h.unpack()))
    }

    fn get_tip_header(&self, verbosity: Option<Uint32>) -> Result<ResponseFormat<HeaderView>> {
        let verbosity = verbosity
            .map(|v| v.value())
            .unwrap_or(DEFAULT_HEADER_VERBOSITY_LEVEL);
        if verbosity == 1 {
            Ok(ResponseFormat::json(
                self.shared.snapshot().tip_header().clone().into(),
            ))
        } else if verbosity == 0 {
            Ok(ResponseFormat::hex(
                self.shared.snapshot().tip_header().data().as_bytes(),
            ))
        } else {
            Err(RPCError::invalid_params("invalid verbosity level"))
        }
    }

    fn get_current_epoch(&self) -> Result<EpochView> {
        Ok(EpochView::from_ext(
            self.shared.snapshot().epoch_ext().pack(),
        ))
    }

    fn get_epoch_by_number(&self, epoch_number: EpochNumber) -> Result<Option<EpochView>> {
        let snapshot = self.shared.snapshot();
        Ok(snapshot
            .get_epoch_index(epoch_number.into())
            .and_then(|hash| {
                snapshot
                    .get_epoch_ext(&hash)
                    .map(|ext| EpochView::from_ext(ext.pack()))
            }))
    }

    fn get_live_cell(&self, out_point: OutPoint, with_data: bool) -> Result<CellWithStatus> {
        let cell_status = self
            .shared
            .snapshot()
            .as_ref()
            .cell(&out_point.into(), with_data);
        Ok(cell_status.into())
    }

    fn get_tip_block_number(&self) -> Result<BlockNumber> {
        Ok(self.shared.snapshot().tip_header().number().into())
    }

    fn get_block_economic_state(&self, block_hash: H256) -> Result<Option<BlockEconomicState>> {
        let snapshot = self.shared.snapshot();

        let block_number = if let Some(block_number) = snapshot.get_block_number(&block_hash.pack())
        {
            block_number
        } else {
            return Ok(None);
        };

        let delay_length = snapshot.consensus().finalization_delay_length();
        let finalized_at_number = block_number + delay_length;
        if block_number == 0 || snapshot.tip_number() < finalized_at_number {
            return Ok(None);
        }

        let block_hash = block_hash.pack();
        let finalized_at = if let Some(block_hash) = snapshot.get_block_hash(finalized_at_number) {
            block_hash
        } else {
            return Ok(None);
        };

        let issuance = if let Some(issuance) = snapshot
            .get_block_epoch_index(&block_hash)
            .and_then(|index| snapshot.get_epoch_ext(&index))
            .and_then(|epoch_ext| {
                let primary = epoch_ext.block_reward(block_number).ok()?;
                let secondary = epoch_ext
                    .secondary_block_issuance(
                        block_number,
                        snapshot.consensus().secondary_epoch_reward(),
                    )
                    .ok()?;
                Some(core::BlockIssuance { primary, secondary })
            }) {
            issuance
        } else {
            return Ok(None);
        };

        let txs_fee = if let Some(txs_fee) =
            snapshot.get_block_ext(&block_hash).and_then(|block_ext| {
                block_ext
                    .txs_fees
                    .iter()
                    .try_fold(core::Capacity::zero(), |acc, tx_fee| acc.safe_add(*tx_fee))
                    .ok()
            }) {
            txs_fee
        } else {
            return Ok(None);
        };

        Ok(snapshot.get_block_header(&block_hash).and_then(|header| {
            RewardCalculator::new(snapshot.consensus(), snapshot.as_ref())
                .block_reward_for_target(&header)
                .ok()
                .map(|(_, block_reward)| core::BlockEconomicState {
                    issuance,
                    miner_reward: block_reward.into(),
                    txs_fee,
                    finalized_at,
                })
                .map(Into::into)
        }))
    }

    fn get_transaction_proof(
        &self,
        tx_hashes: Vec<H256>,
        block_hash: Option<H256>,
    ) -> Result<TransactionProof> {
        let (block, leaf_indices) = self.get_tx_indices(tx_hashes, block_hash)?;
        Ok(TransactionProof {
            block_hash: block.hash().unpack(),
            witnesses_root: block.calc_witnesses_root().unpack(),
            proof: CBMT::build_merkle_proof(
                &block
                    .transactions()
                    .iter()
                    .map(|tx| tx.hash())
                    .collect::<Vec<_>>(),
                &leaf_indices,
            )
            .expect("build proof with verified inputs should be OK")
            .into(),
        })
    }

    fn verify_transaction_proof(&self, tx_proof: TransactionProof) -> Result<Vec<H256>> {
        let snapshot = self.shared.snapshot();

        snapshot
            .get_block(&tx_proof.block_hash.pack())
            .ok_or_else(|| {
                RPCError::invalid_params(format!("Cannot find block {:#x}", tx_proof.block_hash))
            })
            .and_then(|block| {
                let witnesses_root = tx_proof.witnesses_root.pack();
                let merkle_proof = MerkleProof::new(
                    tx_proof
                        .proof
                        .indices
                        .into_iter()
                        .map(|index| index.value())
                        .collect(),
                    tx_proof
                        .proof
                        .lemmas
                        .into_iter()
                        .map(|lemma| lemma.pack())
                        .collect(),
                );

                CBMT::retrieve_leaves(block.tx_hashes(), &merkle_proof)
                    .and_then(|tx_hashes| {
                        merkle_proof
                            .root(&tx_hashes)
                            .and_then(|raw_transactions_root| {
                                if block.transactions_root()
                                    == merkle_root(&[raw_transactions_root, witnesses_root])
                                {
                                    Some(tx_hashes.iter().map(|hash| hash.unpack()).collect())
                                } else {
                                    None
                                }
                            })
                    })
                    .ok_or_else(|| RPCError::invalid_params("Invalid transaction proof"))
            })
    }

    fn get_transaction_and_witness_proof(
        &self,
        tx_hashes: Vec<H256>,
        block_hash: Option<H256>,
    ) -> Result<TransactionAndWitnessProof> {
        let (block, leaf_indices) = self.get_tx_indices(tx_hashes, block_hash)?;
        Ok(TransactionAndWitnessProof {
            block_hash: block.hash().unpack(),
            transactions_proof: CBMT::build_merkle_proof(
                &block
                    .transactions()
                    .iter()
                    .map(|tx| tx.hash())
                    .collect::<Vec<_>>(),
                &leaf_indices,
            )
            .expect("build proof with verified inputs should be OK")
            .into(),
            witnesses_proof: CBMT::build_merkle_proof(block.tx_witness_hashes(), &leaf_indices)
                .expect("build proof with verified inputs should be OK")
                .into(),
        })
    }

    fn verify_transaction_and_witness_proof(
        &self,
        tx_proof: TransactionAndWitnessProof,
    ) -> Result<Vec<H256>> {
        let snapshot = self.shared.snapshot();
        snapshot
            .get_block(&tx_proof.block_hash.pack())
            .ok_or_else(|| {
                RPCError::invalid_params(format!("Cannot find block {:#x}", tx_proof.block_hash))
            })
            .and_then(|block| {
                let transactions_merkle_proof = MerkleProof::new(
                    tx_proof
                        .transactions_proof
                        .indices
                        .into_iter()
                        .map(|index| index.value())
                        .collect(),
                    tx_proof
                        .transactions_proof
                        .lemmas
                        .into_iter()
                        .map(|lemma| lemma.pack())
                        .collect(),
                );
                let witnesses_merkle_proof = MerkleProof::new(
                    tx_proof
                        .witnesses_proof
                        .indices
                        .into_iter()
                        .map(|index| index.value())
                        .collect(),
                    tx_proof
                        .witnesses_proof
                        .lemmas
                        .into_iter()
                        .map(|lemma| lemma.pack())
                        .collect(),
                );

                CBMT::retrieve_leaves(block.tx_witness_hashes(), &witnesses_merkle_proof)
                    .and_then(|witnesses_hashes| witnesses_merkle_proof.root(&witnesses_hashes))
                    .and_then(|witnesses_proof_root| {
                        CBMT::retrieve_leaves(block.tx_hashes(), &transactions_merkle_proof)
                            .and_then(|tx_hashes| {
                                transactions_merkle_proof.root(&tx_hashes).and_then(
                                    |raw_transactions_root| {
                                        if block.transactions_root()
                                            == merkle_root(&[
                                                raw_transactions_root,
                                                witnesses_proof_root,
                                            ])
                                        {
                                            Some(
                                                tx_hashes
                                                    .iter()
                                                    .map(|hash| hash.unpack())
                                                    .collect(),
                                            )
                                        } else {
                                            None
                                        }
                                    },
                                )
                            })
                    })
                    .ok_or_else(|| {
                        RPCError::invalid_params("Invalid transaction_and_witness proof")
                    })
            })
    }

    fn get_fork_block(
        &self,
        block_hash: H256,
        verbosity: Option<Uint32>,
    ) -> Result<Option<ResponseFormat<BlockView>>> {
        let snapshot = self.shared.snapshot();
        let block_hash = block_hash.pack();
        if snapshot.is_main_chain(&block_hash) {
            return Ok(None);
        }

        let verbosity = verbosity
            .map(|v| v.value())
            .unwrap_or(DEFAULT_BLOCK_VERBOSITY_LEVEL);
        // TODO: verbosity level == 1, output block only contains tx_hash in JSON format
        if verbosity == 2 {
            Ok(snapshot
                .get_block(&block_hash)
                .map(|block| ResponseFormat::json(block.into())))
        } else if verbosity == 0 {
            Ok(snapshot
                .get_packed_block(&block_hash)
                .map(|packed| ResponseFormat::hex(packed.as_bytes())))
        } else {
            Err(RPCError::invalid_params("invalid verbosity level"))
        }
    }

    fn get_consensus(&self) -> Result<Consensus> {
        let consensus = self.shared.consensus().clone();
        Ok(consensus.into())
    }

    fn get_block_median_time(&self, block_hash: H256) -> Result<Option<Timestamp>> {
        let block_hash = block_hash.pack();
        let snapshot = self.shared.snapshot();
        if !snapshot.is_main_chain(&block_hash) {
            return Ok(None);
        }

        let median_time = snapshot.block_median_time(
            &block_hash,
            self.shared.consensus().median_time_block_count(),
        );
        Ok(Some(median_time.into()))
    }

    fn estimate_cycles(&self, tx: Transaction) -> Result<EstimateCycles> {
        let tx: packed::Transaction = tx.into();
        CyclesEstimator::new(&self.shared).run(tx)
    }

    fn get_fee_rate_statics(&self, target: Option<Uint64>) -> Result<Option<FeeRateStatistics>> {
        Ok(FeeRateCollector::new(self.shared.snapshot().as_ref())
            .statistics(target.map(Into::into)))
    }

    fn get_fee_rate_statistics(&self, target: Option<Uint64>) -> Result<Option<FeeRateStatistics>> {
        Ok(FeeRateCollector::new(self.shared.snapshot().as_ref())
            .statistics(target.map(Into::into)))
    }
}

impl ChainRpcImpl {
    fn get_transaction_verbosity1(
        &self,
        tx_hash: packed::Byte32,
        only_committed: bool,
    ) -> Result<TransactionWithStatus> {
        let snapshot = self.shared.snapshot();
        if let Some(tx_info) = snapshot.get_transaction_info(&tx_hash) {
            let cycles = if tx_info.is_cellbase() {
                None
            } else {
                snapshot
                    .get_block_ext(&tx_info.block_hash)
                    .and_then(|block_ext| {
                        block_ext
                            .cycles
                            .and_then(|v| v.get(tx_info.index.saturating_sub(1)).copied())
                    })
            };

            return Ok(TransactionWithStatus::with_committed(
                None,
                tx_info.block_hash.unpack(),
                cycles,
            ));
        }

        if only_committed {
            return Ok(TransactionWithStatus::with_unknown());
        }

        let tx_pool = self.shared.tx_pool_controller();
        let tx_status = tx_pool.get_tx_status(tx_hash);
        if let Err(e) = tx_status {
            error!("send get_tx_status request error {}", e);
            return Err(RPCError::ckb_internal_error(e));
        };
        let tx_status = tx_status.unwrap();

        if let Err(e) = tx_status {
            error!("get_tx_status from db error {}", e);
            return Err(RPCError::ckb_internal_error(e));
        };
        let (tx_status, cycles) = tx_status.unwrap();
        Ok(TransactionWithStatus::omit_transaction(tx_status, cycles))
    }

    fn get_transaction_verbosity2(
        &self,
        tx_hash: packed::Byte32,
        only_committed: bool,
    ) -> Result<TransactionWithStatus> {
        let snapshot = self.shared.snapshot();
        if let Some((tx, tx_info)) = snapshot.get_transaction_with_info(&tx_hash) {
            let cycles = if tx_info.is_cellbase() {
                None
            } else {
                snapshot
                    .get_block_ext(&tx_info.block_hash)
                    .and_then(|block_ext| {
                        block_ext
                            .cycles
                            .and_then(|v| v.get(tx_info.index.saturating_sub(1)).copied())
                    })
            };

            return Ok(TransactionWithStatus::with_committed(
                Some(tx),
                tx_info.block_hash.unpack(),
                cycles,
            ));
        }

        if only_committed {
            return Ok(TransactionWithStatus::with_unknown());
        }

        let tx_pool = self.shared.tx_pool_controller();
        let transaction_with_status = tx_pool.get_transaction_with_status(tx_hash);
        if let Err(e) = transaction_with_status {
            error!("send get_transaction_with_status request error {}", e);
            return Err(RPCError::ckb_internal_error(e));
        };
        let transaction_with_status = transaction_with_status.unwrap();

        if let Err(e) = transaction_with_status {
            error!("get transaction_with_status from db error {}", e);
            return Err(RPCError::ckb_internal_error(e));
        };
        let transaction_with_status = transaction_with_status.unwrap();
        Ok(transaction_with_status)
    }
    fn get_block_by_hash(
        &self,
        snapshot: &Snapshot,
        block_hash: &packed::Byte32,
        verbosity: Option<Uint32>,
        with_cycles: Option<bool>,
    ) -> Result<Option<BlockResponse>> {
        if !snapshot.is_main_chain(block_hash) {
            return Ok(None);
        }

        let verbosity = verbosity
            .map(|v| v.value())
            .unwrap_or(DEFAULT_BLOCK_VERBOSITY_LEVEL);

        // default false
        let with_cycles = with_cycles.unwrap_or(false);

        // TODO: verbosity level == 1, output block only contains tx_hash in JSON format
        let block_view = if verbosity == 2 {
            snapshot
                .get_block(block_hash)
                .map(|block| ResponseFormat::json(block.into()))
        } else if verbosity == 0 {
            snapshot
                .get_packed_block(block_hash)
                .map(|packed| ResponseFormat::hex(packed.as_bytes()))
        } else {
            return Err(RPCError::invalid_params("invalid verbosity level"));
        };

        Ok(block_view.map(|block| {
            if with_cycles {
                let cycles = snapshot
                    .get_block_ext(block_hash)
                    .and_then(|ext| ext.cycles);

                BlockResponse::with_cycles(
                    block,
                    cycles.map(|c| c.into_iter().map(Into::into).collect()),
                )
            } else {
                BlockResponse::regular(block)
            }
        }))
    }

    fn get_tx_indices(
        &self,
        tx_hashes: Vec<H256>,
        block_hash: Option<H256>,
    ) -> Result<(core::BlockView, Vec<u32>)> {
        if tx_hashes.is_empty() {
            return Err(RPCError::invalid_params("Empty transaction hashes"));
        }
        let snapshot = self.shared.snapshot();

        let mut retrieved_block_hash = None;
        let mut tx_indices = HashSet::new();
        for tx_hash in tx_hashes {
            match snapshot.get_transaction_info(&tx_hash.pack()) {
                Some(tx_info) => {
                    if retrieved_block_hash.is_none() {
                        retrieved_block_hash = Some(tx_info.block_hash);
                    } else if Some(tx_info.block_hash) != retrieved_block_hash {
                        return Err(RPCError::invalid_params(
                            "Not all transactions found in retrieved block",
                        ));
                    }

                    if !tx_indices.insert(tx_info.index as u32) {
                        return Err(RPCError::invalid_params(format!(
                            "Duplicated tx_hash {tx_hash:#x}"
                        )));
                    }
                }
                None => {
                    return Err(RPCError::invalid_params(format!(
                        "Transaction {tx_hash:#x} not yet in block"
                    )));
                }
            }
        }

        let retrieved_block_hash = retrieved_block_hash.expect("checked len");
        if let Some(specified_block_hash) = block_hash {
            if !retrieved_block_hash.eq(&specified_block_hash.pack()) {
                return Err(RPCError::invalid_params(
                    "Not all transactions found in specified block",
                ));
            }
        }

        snapshot
            .get_block(&retrieved_block_hash)
            .ok_or_else(|| {
                let message = format!(
                    "Chain TransactionInfo says block {retrieved_block_hash:#x} existing, but that block is not in the database"
                );
                error!("{}", message);
                RPCError::custom(RPCError::ChainIndexIsInconsistent, message)
            })
            .map(|block| {
                (block, tx_indices.into_iter().collect::<Vec<_>>())
            })
    }
}

// CyclesEstimator run given transaction, and return the result, including execution cycles.
pub(crate) struct CyclesEstimator<'a> {
    shared: &'a Shared,
}

impl<'a> CellProvider for CyclesEstimator<'a> {
    fn cell(&self, out_point: &packed::OutPoint, eager_load: bool) -> CellStatus {
        let snapshot = self.shared.snapshot();
        snapshot
            .get_cell(out_point)
            .map(|mut cell_meta| {
                if eager_load {
                    if let Some((data, data_hash)) = snapshot.get_cell_data(out_point) {
                        cell_meta.mem_cell_data = Some(data);
                        cell_meta.mem_cell_data_hash = Some(data_hash);
                    }
                }
                CellStatus::live_cell(cell_meta)
            })  // treat as live cell, regardless of live or dead
            .unwrap_or(CellStatus::Unknown)
    }
}

impl<'a> HeaderChecker for CyclesEstimator<'a> {
    fn check_valid(&self, block_hash: &packed::Byte32) -> std::result::Result<(), OutPointError> {
        self.shared.snapshot().check_valid(block_hash)
    }
}

impl<'a> CyclesEstimator<'a> {
    pub(crate) fn new(shared: &'a Shared) -> Self {
        Self { shared }
    }

    pub(crate) fn run(&self, tx: packed::Transaction) -> Result<EstimateCycles> {
        let snapshot = self.shared.cloned_snapshot();
        let consensus = snapshot.cloned_consensus();
        match resolve_transaction(tx.into_view(), &mut HashSet::new(), self, self) {
            Ok(resolved) => {
                let max_cycles = consensus.max_block_cycles;
                let tip_header = snapshot.tip_header();
                let tx_env = TxVerifyEnv::new_submit(tip_header);
                match ScriptVerifier::new(
                    Arc::new(resolved),
                    snapshot.as_data_loader(),
                    consensus,
                    Arc::new(tx_env),
                )
                .verify(max_cycles)
                {
                    Ok(cycles) => Ok(EstimateCycles {
                        cycles: cycles.into(),
                    }),
                    Err(err) => Err(RPCError::custom_with_error(
                        RPCError::TransactionFailedToVerify,
                        err,
                    )),
                }
            }
            Err(err) => Err(RPCError::custom_with_error(
                RPCError::TransactionFailedToResolve,
                err,
            )),
        }
    }
}
