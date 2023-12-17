use crate::error::RPCError;
use async_trait::async_trait;
use ckb_indexer_r::AsyncIndexerRHandle;
use ckb_jsonrpc_types::IndexerTip;
use jsonrpc_core::Result;
use jsonrpc_utils::rpc;

/// RPC Module Indexer.
#[rpc]
#[async_trait]
pub trait IndexerRRpc {
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
    ///     "method": "get_indexer_r_tip"
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
    #[rpc(name = "get_indexer_r_tip")]
    async fn get_indexer_r_tip(&self) -> Result<Option<IndexerTip>>;
}

#[derive(Clone)]
pub(crate) struct IndexerRRpcImpl {
    pub(crate) handle: AsyncIndexerRHandle,
}

impl IndexerRRpcImpl {
    pub fn new(handle: AsyncIndexerRHandle) -> Self {
        IndexerRRpcImpl { handle }
    }
}

#[async_trait]
impl IndexerRRpc for IndexerRRpcImpl {
    async fn get_indexer_r_tip(&self) -> Result<Option<IndexerTip>> {
        self.handle
            .query_indexer_tip()
            .await
            .map_err(|e| RPCError::custom(RPCError::Indexer, e))
    }
}
