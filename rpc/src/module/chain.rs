use crate::error::RPCError;
use ckb_jsonrpc_types::{
    BlockEconomicState, BlockNumber, BlockReward, BlockView, CellOutputWithOutPoint,
    CellWithStatus, EpochNumber, EpochView, HeaderView, MerkleProof as JsonMerkleProof, OutPoint,
    ResponseFormat, TransactionProof, TransactionWithStatus, Uint32,
};
use ckb_logger::error;
use ckb_reward_calculator::RewardCalculator;
use ckb_shared::shared::Shared;
use ckb_store::ChainStore;
use ckb_types::{
    core::{self, cell::CellProvider},
    packed::{self, Block, Header},
    prelude::*,
    utilities::{merkle_root, MerkleProof, CBMT},
    H256,
};
use jsonrpc_core::Result;
use jsonrpc_derive::rpc;
use std::collections::HashSet;

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
    ///     "header": {
    ///       "compact_target": "0x1e083126",
    ///       "dao": "0xb5a3e047474401001bc476b9ee573000c0c387962a38000000febffacf030000",
    ///       "epoch": "0x7080018000001",
    ///       "hash": "0xa5f5c85987a15de25661e5a214f2c1449cd803f071acc7999820f25246471f40",
    ///       "nonce": "0x0",
    ///       "number": "0x400",
    ///       "parent_hash": "0xae003585fa15309b30b31aed3dcf385e9472c3c3e93746a6c4540629a6a1ed2d",
    ///       "proposals_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
    ///       "timestamp": "0x5cd2b117",
    ///       "transactions_root": "0xc47d5b78b3c4c4c853e2a32810818940d0ee403423bea9ec7b8e566d9595206c",
    ///       "uncles_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
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
    ///               "args": "0x",
    ///               "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
    ///               "hash_type": "data"
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
    #[rpc(name = "get_block")]
    fn get_block(
        &self,
        block_hash: H256,
        verbosity: Option<Uint32>,
    ) -> Result<Option<ResponseFormat<BlockView, Block>>>;

    /// Returns the block in the [canonical chain](#canonical-chain) with the specific block number.
    ///
    /// ## Params
    ///
    /// * `block_number` - the block number.
    /// * `verbosity` - result format which allows 0 and 2. (**Optional**, the default is 2.)
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
    ///       "hash": "0xa5f5c85987a15de25661e5a214f2c1449cd803f071acc7999820f25246471f40",
    ///       "nonce": "0x0",
    ///       "number": "0x400",
    ///       "parent_hash": "0xae003585fa15309b30b31aed3dcf385e9472c3c3e93746a6c4540629a6a1ed2d",
    ///       "proposals_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
    ///       "timestamp": "0x5cd2b117",
    ///       "transactions_root": "0xc47d5b78b3c4c4c853e2a32810818940d0ee403423bea9ec7b8e566d9595206c",
    ///       "uncles_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
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
    ///               "args": "0x",
    ///               "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
    ///               "hash_type": "data"
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
    #[rpc(name = "get_block_by_number")]
    fn get_block_by_number(
        &self,
        block_number: BlockNumber,
        verbosity: Option<Uint32>,
    ) -> Result<Option<ResponseFormat<BlockView, Block>>>;

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
    ///     "hash": "0xa5f5c85987a15de25661e5a214f2c1449cd803f071acc7999820f25246471f40",
    ///     "nonce": "0x0",
    ///     "number": "0x400",
    ///     "parent_hash": "0xae003585fa15309b30b31aed3dcf385e9472c3c3e93746a6c4540629a6a1ed2d",
    ///     "proposals_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
    ///     "timestamp": "0x5cd2b117",
    ///     "transactions_root": "0xc47d5b78b3c4c4c853e2a32810818940d0ee403423bea9ec7b8e566d9595206c",
    ///     "uncles_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
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
    ) -> Result<Option<ResponseFormat<HeaderView, Header>>>;

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
    ///     "hash": "0xa5f5c85987a15de25661e5a214f2c1449cd803f071acc7999820f25246471f40",
    ///     "nonce": "0x0",
    ///     "number": "0x400",
    ///     "parent_hash": "0xae003585fa15309b30b31aed3dcf385e9472c3c3e93746a6c4540629a6a1ed2d",
    ///     "proposals_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
    ///     "timestamp": "0x5cd2b117",
    ///     "transactions_root": "0xc47d5b78b3c4c4c853e2a32810818940d0ee403423bea9ec7b8e566d9595206c",
    ///     "uncles_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
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
    ) -> Result<Option<ResponseFormat<HeaderView, Header>>>;

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
    ///             "args": "0x",
    ///             "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
    ///             "hash_type": "data"
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
    ///     "tx_status": {
    ///       "block_hash": null,
    ///       "status": "pending"
    ///     }
    ///   }
    /// }
    /// ```
    #[rpc(name = "get_transaction")]
    fn get_transaction(&self, tx_hash: H256) -> Result<Option<TransactionWithStatus>>;

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
    ///     "hash": "0xa5f5c85987a15de25661e5a214f2c1449cd803f071acc7999820f25246471f40",
    ///     "nonce": "0x0",
    ///     "number": "0x400",
    ///     "parent_hash": "0xae003585fa15309b30b31aed3dcf385e9472c3c3e93746a6c4540629a6a1ed2d",
    ///     "proposals_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
    ///     "timestamp": "0x5cd2b117",
    ///     "transactions_root": "0xc47d5b78b3c4c4c853e2a32810818940d0ee403423bea9ec7b8e566d9595206c",
    ///     "uncles_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
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
    fn get_tip_header(
        &self,
        verbosity: Option<Uint32>,
    ) -> Result<ResponseFormat<HeaderView, Header>>;

    /// Returns the information about [live cell](#live-cell)s collection by the hash of lock script.
    ///
    /// This method will be removed. It always returns an error now.
    #[deprecated(
        since = "0.36.0",
        note = "(Disabled since 0.36.0) This method is deprecated for reasons of flexibility.
        Please use [ckb-indexer](https://github.com/nervosnetwork/ckb-indexer) as an alternate
        solution"
    )]
    #[rpc(name = "deprecated.get_cells_by_lock_hash")] // noexample
    fn get_cells_by_lock_hash(
        &self,
        lock_hash: H256,
        from: BlockNumber,
        to: BlockNumber,
    ) -> Result<Vec<CellOutputWithOutPoint>>;

    /// Returns the status of a cell. The RPC returns extra information if it is a [live cell]
    /// (#live-cell).
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
    ///           "args": "0x",
    ///           "code_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
    ///           "hash_type": "data"
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

    /// Returns each component of the created CKB in the block's cellbase.
    ///
    /// This RPC returns null if the block is not in the [canonical chain](#canonical-chain).
    ///
    /// CKB delays CKB creation for miners. The output cells in the cellbase of block N are for the
    /// miner creating block `N - 1 - ProposalWindow.farthest`.
    ///
    /// In mainnet, `ProposalWindow.farthest` is 10, so the outputs in block 100 are rewards for
    /// miner creating block 89.
    ///
    /// ## Params
    ///
    /// * `block_hash` - Specifies the block hash which cellbase outputs should be analyzed.
    ///
    /// ## Returns
    ///
    /// If the block with the hash `block_hash` is in the [canonical chain](#canonical-chain) and
    /// its block number is N, return the block rewards analysis for block `N - 1 - ProposalWindow.farthest`.
    ///
    /// ## Examples
    ///
    /// Request
    ///
    /// ```json
    /// {
    ///   "id": 42,
    ///   "jsonrpc": "2.0",
    ///   "method": "get_cellbase_output_capacity_details",
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
    ///     "primary": "0x18ce922bca",
    ///     "proposal_reward": "0x0",
    ///     "secondary": "0x17b93605",
    ///     "total": "0x18e64b61cf",
    ///     "tx_fee": "0x0"
    ///   }
    /// }
    /// ```
    #[deprecated(
        since = "0.36.0",
        note = "Please use the RPC method [`get_block_economic_state`](#tymethod.get_block_economic_state) instead"
    )]
    #[rpc(name = "get_cellbase_output_capacity_details")]
    fn get_cellbase_output_capacity_details(&self, block_hash: H256)
        -> Result<Option<BlockReward>>;

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
    /// its rewards have been finalized, return the block rewards analysis for this block.
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
    ///       "hash": "0xdca341a42890536551f99357612cef7148ed471e3b6419d0844a4e400be6ee94",
    ///       "nonce": "0x0",
    ///       "number": "0x400",
    ///       "parent_hash": "0xae003585fa15309b30b31aed3dcf385e9472c3c3e93746a6c4540629a6a1ed2d",
    ///       "proposals_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
    ///       "timestamp": "0x5cd2b118",
    ///       "transactions_root": "0xc47d5b78b3c4c4c853e2a32810818940d0ee403423bea9ec7b8e566d9595206c",
    ///       "uncles_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
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
    ///               "args": "0x",
    ///               "code_hash": "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5",
    ///               "hash_type": "data"
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
    ) -> Result<Option<ResponseFormat<BlockView, Block>>>;
}

pub(crate) struct ChainRpcImpl {
    pub shared: Shared,
}

const DEFAULT_BLOCK_VERBOSITY_LEVEL: u32 = 2;
const DEFAULT_HEADER_VERBOSITY_LEVEL: u32 = 1;

impl ChainRpc for ChainRpcImpl {
    fn get_block(
        &self,
        block_hash: H256,
        verbosity: Option<Uint32>,
    ) -> Result<Option<ResponseFormat<BlockView, Block>>> {
        let snapshot = self.shared.snapshot();
        let block_hash = block_hash.pack();
        if !snapshot.is_main_chain(&block_hash) {
            return Ok(None);
        }

        let verbosity = verbosity
            .map(|v| v.value())
            .unwrap_or(DEFAULT_BLOCK_VERBOSITY_LEVEL);
        // TODO: verbosity level == 1, output block only contains tx_hash in JSON format
        if verbosity == 2 {
            Ok(snapshot
                .get_block(&block_hash)
                .map(|block| ResponseFormat::Json(block.into())))
        } else if verbosity == 0 {
            Ok(snapshot
                .get_packed_block(&block_hash)
                .map(ResponseFormat::Hex))
        } else {
            Err(RPCError::invalid_params("invalid verbosity level"))
        }
    }

    fn get_block_by_number(
        &self,
        block_number: BlockNumber,
        verbosity: Option<Uint32>,
    ) -> Result<Option<ResponseFormat<BlockView, Block>>> {
        let snapshot = self.shared.snapshot();
        let block_hash = match snapshot.get_block_hash(block_number.into()) {
            Some(block_hash) => block_hash,
            None => return Ok(None),
        };

        let verbosity = verbosity
            .map(|v| v.value())
            .unwrap_or(DEFAULT_BLOCK_VERBOSITY_LEVEL);
        // TODO: verbosity level == 1, output block only contains tx_hash in json format
        let result = if verbosity == 2 {
            snapshot
                .get_block(&block_hash)
                .map(|block| Some(ResponseFormat::Json(block.into())))
        } else if verbosity == 0 {
            snapshot
                .get_packed_block(&block_hash)
                .map(|block| Some(ResponseFormat::Hex(block)))
        } else {
            return Err(RPCError::invalid_params("invalid verbosity level"));
        };

        result.ok_or_else(|| {
            let message = format!(
                "Chain Index says block #{} is {:#x}, but that block is not in the database",
                block_number, block_hash
            );
            error!("{}", message);
            RPCError::custom(RPCError::ChainIndexIsInconsistent, message)
        })
    }

    fn get_header(
        &self,
        block_hash: H256,
        verbosity: Option<Uint32>,
    ) -> Result<Option<ResponseFormat<HeaderView, Header>>> {
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
                .map(|header| ResponseFormat::Json(header.into())))
        } else if verbosity == 0 {
            Ok(snapshot
                .get_packed_block_header(&block_hash)
                .map(ResponseFormat::Hex))
        } else {
            Err(RPCError::invalid_params("invalid verbosity level"))
        }
    }

    fn get_header_by_number(
        &self,
        block_number: BlockNumber,
        verbosity: Option<Uint32>,
    ) -> Result<Option<ResponseFormat<HeaderView, Header>>> {
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
                .map(|header| Some(ResponseFormat::Json(header.into())))
        } else if verbosity == 0 {
            snapshot
                .get_packed_block_header(&block_hash)
                .map(|header| Some(ResponseFormat::Hex(header)))
        } else {
            return Err(RPCError::invalid_params("invalid verbosity level"));
        };

        result.ok_or_else(|| {
            let message = format!(
                "Chain Index says block #{} is {:#x}, but that block is not in the database",
                block_number, block_hash
            );
            error!("{}", message);
            RPCError::custom(RPCError::ChainIndexIsInconsistent, message)
        })
    }

    fn get_transaction(&self, tx_hash: H256) -> Result<Option<TransactionWithStatus>> {
        let tx_hash = tx_hash.pack();
        let id = packed::ProposalShortId::from_tx_hash(&tx_hash);

        let tx = {
            let tx_pool = self.shared.tx_pool_controller();
            let fetch_tx_for_rpc = tx_pool.fetch_tx_for_rpc(id);
            if let Err(e) = fetch_tx_for_rpc {
                error!("send fetch_tx_for_rpc request error {}", e);
                return Err(RPCError::ckb_internal_error(e));
            };

            fetch_tx_for_rpc.unwrap().map(|(proposed, tx)| {
                if proposed {
                    TransactionWithStatus::with_proposed(tx)
                } else {
                    TransactionWithStatus::with_pending(tx)
                }
            })
        };

        Ok(tx.or_else(|| {
            self.shared
                .snapshot()
                .get_transaction(&tx_hash)
                .map(|(tx, block_hash)| {
                    TransactionWithStatus::with_committed(tx, block_hash.unpack())
                })
        }))
    }

    fn get_block_hash(&self, block_number: BlockNumber) -> Result<Option<H256>> {
        Ok(self
            .shared
            .snapshot()
            .get_block_hash(block_number.into())
            .map(|h| h.unpack()))
    }

    fn get_tip_header(
        &self,
        verbosity: Option<Uint32>,
    ) -> Result<ResponseFormat<HeaderView, Header>> {
        let verbosity = verbosity
            .map(|v| v.value())
            .unwrap_or(DEFAULT_HEADER_VERBOSITY_LEVEL);
        if verbosity == 1 {
            Ok(ResponseFormat::Json(
                self.shared.snapshot().tip_header().clone().into(),
            ))
        } else if verbosity == 0 {
            Ok(ResponseFormat::Hex(
                self.shared.snapshot().tip_header().data(),
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

    fn get_cells_by_lock_hash(
        &self,
        _lock_hash: H256,
        _from: BlockNumber,
        _to: BlockNumber,
    ) -> Result<Vec<CellOutputWithOutPoint>> {
        Err(RPCError::custom(
            RPCError::Invalid,
            "get_cells_by_lock_hash have been deprecated, use [indexer] get_live_cells_by_lock_hash instead",
        ))
    }

    fn get_live_cell(&self, out_point: OutPoint, with_data: bool) -> Result<CellWithStatus> {
        let cell_status = self.shared.snapshot().cell(&out_point.into(), with_data);
        Ok(cell_status.into())
    }

    fn get_tip_block_number(&self) -> Result<BlockNumber> {
        Ok(self.shared.snapshot().tip_header().number().into())
    }

    fn get_cellbase_output_capacity_details(
        &self,
        block_hash: H256,
    ) -> Result<Option<BlockReward>> {
        let snapshot = self.shared.snapshot();

        if !snapshot.is_main_chain(&block_hash.pack()) {
            return Ok(None);
        }

        Ok(snapshot
            .get_block_header(&block_hash.pack())
            .and_then(|header| {
                snapshot
                    .get_block_header(&header.data().raw().parent_hash())
                    .and_then(|parent| {
                        if parent.number() < snapshot.consensus().finalization_delay_length() {
                            None
                        } else {
                            RewardCalculator::new(snapshot.consensus(), snapshot.as_ref())
                                .block_reward_to_finalize(&parent)
                                .map(|r| r.1.into())
                                .ok()
                        }
                    })
            }))
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
                            "Duplicated tx_hash {:#x}",
                            tx_hash
                        )));
                    }
                }
                None => {
                    return Err(RPCError::invalid_params(format!(
                        "Transaction {:#x} not yet in block",
                        tx_hash
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
                    "Chain TransactionInfo says block {:#x} existing, but that block is not in the database",
                    retrieved_block_hash
                );
                error!("{}", message);
                RPCError::custom(RPCError::ChainIndexIsInconsistent, message)
            })
            .map(|block| {
                let proof = CBMT::build_merkle_proof(
                    &block.transactions().iter().map(|tx| tx.hash()).collect::<Vec<_>>(),
                    &tx_indices.into_iter().collect::<Vec<_>>())
                .expect("build proof with verified inputs should be OK");
                TransactionProof {
                    block_hash: block.hash().unpack(),
                    witnesses_root: block.calc_witnesses_root().unpack(),
                    proof: JsonMerkleProof {
                        indices: proof.indices().iter().map(|index| (*index).into()).collect(),
                        lemmas: proof.lemmas().iter().map(|lemma| Unpack::<H256>::unpack(lemma)).collect(),
                    }
                }
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

                CBMT::retrieve_leaves(&block.tx_hashes(), &merkle_proof)
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

    fn get_fork_block(
        &self,
        block_hash: H256,
        verbosity: Option<Uint32>,
    ) -> Result<Option<ResponseFormat<BlockView, Block>>> {
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
                .map(|block| ResponseFormat::Json(block.into())))
        } else if verbosity == 0 {
            Ok(snapshot
                .get_packed_block(&block_hash)
                .map(ResponseFormat::Hex))
        } else {
            Err(RPCError::invalid_params("invalid verbosity level"))
        }
    }
}
