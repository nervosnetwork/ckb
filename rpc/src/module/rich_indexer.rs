use crate::error::RPCError;
use async_trait::async_trait;
use ckb_jsonrpc_types::{
    IndexerCell, IndexerCellsCapacity, IndexerOrder, IndexerPagination, IndexerSearchKey,
    IndexerTip, IndexerTx, JsonBytes, Uint32,
};
use ckb_rich_indexer::AsyncRichIndexerHandle;
use jsonrpc_core::Result;
use jsonrpc_utils::rpc;

/// RPC Module Rich Indexer.
#[rpc(openrpc)]
#[async_trait]
pub trait RichIndexerRpc {
    /// Returns the indexed tip
    ///
    /// ## Returns
    ///   * block_hash - indexed tip block hash
    ///   * block_number - indexed tip block number
    ///
    /// ## Examples
    ///
    /// Same as CKB Indexer.
    #[rpc(name = "get_indexer_tip")]
    async fn get_indexer_tip(&self) -> Result<Option<IndexerTip>>;

    /// Returns the live cells collection by the lock or type script.
    ///
    /// ## Params
    ///
    /// * search_key:
    ///     - script - Script, supports prefix search
    ///     - script_type - enum, lock | type
    ///     - script_search_mode - enum, prefix | exact | partial
    ///     - filter - filter cells by following conditions, all conditions are optional
    ///          - script: if search script type is lock, filter cells by type script prefix, and vice versa
    ///          - script_len_range: [u64; 2], filter cells by script len range, [inclusive, exclusive]
    ///          - output_data: filter cells by output data
    ///          - output_data_filter_mode: enum, prefix | exact | partial
    ///          - output_data_len_range: [u64; 2], filter cells by output data len range, [inclusive, exclusive]
    ///          - output_capacity_range: [u64; 2], filter cells by output capacity range, [inclusive, exclusive]
    ///          - block_range: [u64; 2], filter cells by block number range, [inclusive, exclusive]
    ///     - with_data - bool, optional default is `true`, if with_data is set to false, the field of returning cell.output_data is null in the result
    /// * order: enum, asc | desc
    /// * limit: result size limit
    /// * after: pagination parameter, optional
    ///
    /// ## Returns
    ///
    /// If the number of objects is less than the requested `limit`, it indicates that these are the last page of get_cells.
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
    /// Same as CKB Indexer.
    #[rpc(name = "get_cells")]
    async fn get_cells(
        &self,
        search_key: IndexerSearchKey,
        order: IndexerOrder,
        limit: Uint32,
        after: Option<JsonBytes>,
    ) -> Result<IndexerPagination<IndexerCell>>;

    /// Returns the transactions collection by the lock or type script.
    ///
    /// * search_key:
    ///     - script - Script, supports prefix search when group_by_transaction is false
    ///     - script_type - enum, lock | type
    ///     - script_search_mode - enum, prefix | exact | partial
    ///     - filter - filter cells by following conditions, all conditions are optional
    ///         - script: if search script type is lock, filter cells by type script, and vice versa
    ///         - script_len_range: [u64; 2], filter cells by script len range, [inclusive, exclusive]
    ///         - output_data: filter cells by output data
    ///         - output_data_filter_mode: enum, prefix | exact | partial
    ///         - output_data_len_range: [u64; 2], filter cells by output data len range, [inclusive, exclusive]
    ///         - output_capacity_range: [u64; 2], filter cells by output capacity range, [inclusive, exclusive]
    ///         - block_range: [u64; 2], filter cells by block number range, [inclusive, exclusive]
    ///     - group_by_transaction - bool, optional default is `false`, if group_by_transaction is set to true, the returning objects will be grouped by the tx hash
    /// * order: enum, asc | desc
    /// * limit: result size limit
    /// * after: pagination parameter, optional
    ///
    /// ## Returns
    ///
    /// If the number of objects is less than the requested `limit`, it indicates that these are the last page of get_transactions.
    ///
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
    /// Same as CKB Indexer.
    #[rpc(name = "get_transactions")]
    async fn get_transactions(
        &self,
        search_key: IndexerSearchKey,
        order: IndexerOrder,
        limit: Uint32,
        after: Option<JsonBytes>,
    ) -> Result<IndexerPagination<IndexerTx>>;

    /// Returns the live cells capacity by the lock or type script.
    ///
    /// ## Parameters
    ///
    /// * search_key:
    ///     - script - Script
    ///     - script_type - enum, lock | type
    ///     - script_search_mode - enum, prefix | exact | partial
    ///     - filter - filter cells by following conditions, all conditions are optional
    ///         - script: if search script type is lock, filter cells by type script prefix, and vice versa
    ///         - script_len_range: [u64; 2], filter cells by script len range, [inclusive, exclusive]
    ///         - output_data: filter cells by output data
    ///         - output_data_filter_mode: enum, prefix | exact | partial
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
    /// Same as CKB Indexer.
    #[rpc(name = "get_cells_capacity")]
    async fn get_cells_capacity(
        &self,
        search_key: IndexerSearchKey,
    ) -> Result<Option<IndexerCellsCapacity>>;
}

#[derive(Clone)]
pub(crate) struct RichIndexerRpcImpl {
    pub(crate) handle: AsyncRichIndexerHandle,
}

impl RichIndexerRpcImpl {
    pub fn new(handle: AsyncRichIndexerHandle) -> Self {
        RichIndexerRpcImpl { handle }
    }
}

#[async_trait]
impl RichIndexerRpc for RichIndexerRpcImpl {
    async fn get_indexer_tip(&self) -> Result<Option<IndexerTip>> {
        self.handle
            .get_indexer_tip()
            .await
            .map_err(|e| RPCError::custom(RPCError::Indexer, e))
    }

    async fn get_cells(
        &self,
        search_key: IndexerSearchKey,
        order: IndexerOrder,
        limit: Uint32,
        after: Option<JsonBytes>,
    ) -> Result<IndexerPagination<IndexerCell>> {
        self.handle
            .get_cells(search_key, order, limit, after)
            .await
            .map_err(|e| RPCError::custom(RPCError::Indexer, e))
    }

    async fn get_transactions(
        &self,
        search_key: IndexerSearchKey,
        order: IndexerOrder,
        limit: Uint32,
        after: Option<JsonBytes>,
    ) -> Result<IndexerPagination<IndexerTx>> {
        self.handle
            .get_transactions(search_key, order, limit, after)
            .await
            .map_err(|e| RPCError::custom(RPCError::Indexer, e))
    }

    async fn get_cells_capacity(
        &self,
        search_key: IndexerSearchKey,
    ) -> Result<Option<IndexerCellsCapacity>> {
        self.handle
            .get_cells_capacity(search_key)
            .await
            .map_err(|e| RPCError::custom(RPCError::Indexer, e))
    }
}
